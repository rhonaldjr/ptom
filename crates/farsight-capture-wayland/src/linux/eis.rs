//! Input through libei: the RemoteDesktop portal's `ConnectToEIS` hands us a
//! socket to the compositor's input emulation server, and we send pointer,
//! button, scroll and key events on the devices it gives us. COSMIC (1.6+)
//! takes remote input only this way; GNOME prefers it too. Without
//! `ConnectToEIS` the portal's per-event D-Bus calls are used instead; once
//! it has been called, those calls are refused, so libei failing later ends
//! the capture rather than silently dropping input.

use std::collections::{HashMap, HashSet};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use reis::ei;
use reis::event::{Connection, Device, DeviceCapability, EiEvent, Region};
use xkbcommon::xkb;

use crate::CaptureError;
use crate::pointer::ScrollAxis;

/// Shift's evdev code (`KEY_LEFTSHIFT`).
const LEFT_SHIFT: u32 = 42;
/// X11 keysyms for the Shift keys.
const SHIFT_KEYSYMS: [u32; 2] = [0xffe1, 0xffe2];
/// X11 keysyms for AltGr (`ISO_Level3_Shift`, `Mode_switch`).
const ALTGR_KEYSYMS: [u32; 2] = [0xfe03, 0xff7e];
/// Largest keymap we read from the compositor.
const MAX_KEYMAP: u32 = 1 << 20;
/// One discrete wheel step in libei units.
const WHEEL_STEP: i32 = 120;
/// How long the compositor gets to complete the libei handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// The connection to the compositor's input emulation, and what it gave us.
pub(super) struct Eis {
    connection: Connection,
    state: Arc<Mutex<State>>,
    /// Our end of the socket, kept to shut it down (which ends the thread).
    socket: UnixStream,
    /// The shared stream's mapping id: picks its region on multi-monitor setups.
    mapping_id: Option<String>,
    /// Modifier keys we pressed for a keysym, released with it.
    synthetic: Mutex<HashMap<u32, Vec<u32>>>,
    started: std::time::Instant,
}

/// Devices the compositor resumed for us (only those may emulate).
#[derive(Default)]
struct State {
    devices: Vec<Device>,
    keys: Option<KeyLookup>,
    sequence: u32,
    /// The compositor disconnected us (or the socket failed).
    closed: bool,
}

impl State {
    fn with<T: ei::Interface>(&self) -> Option<(Device, T)> {
        self.devices
            .iter()
            .find_map(|d| d.interface::<T>().map(|i| (d.clone(), i)))
    }
}

/// Opens the libei connection on `fd` (from `ConnectToEIS`) as a sender.
/// Its event stream isn't `Send`, so it runs on its own thread (with a
/// single-threaded runtime); input is sent from the caller's side.
pub(super) async fn connect(fd: OwnedFd, mapping_id: Option<String>) -> Result<Eis, CaptureError> {
    let failed = |e: &dyn std::fmt::Display| CaptureError::Portal(format!("libei: {e}"));
    let socket = UnixStream::from(fd);
    let ours = socket.try_clone().map_err(|e| failed(&e))?;
    let state = Arc::new(Mutex::new(State::default()));
    let (ready_tx, ready) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("farsight-libei".into())
        .spawn({
            let state = Arc::clone(&state);
            move || run(socket, &state, ready_tx)
        })
        .map_err(|e| failed(&e))?;
    let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, ready).await;
    let connection = match handshake {
        Ok(Ok(Ok(connection))) => connection,
        Ok(Ok(Err(error))) => return Err(failed(&error)),
        Ok(Err(_)) => return Err(failed(&"the libei thread stopped")),
        Err(_) => {
            // Ends the thread's handshake.
            let _ = ours.shutdown(std::net::Shutdown::Both);
            return Err(failed(
                &"the compositor didn't complete the libei handshake",
            ));
        }
    };
    Ok(Eis {
        connection,
        state,
        socket: ours,
        mapping_id,
        synthetic: Mutex::new(HashMap::new()),
        started: std::time::Instant::now(),
    })
}

/// The libei thread: handshake, then device events until the connection ends.
fn run(
    socket: UnixStream,
    state: &Mutex<State>,
    ready: tokio::sync::oneshot::Sender<Result<Connection, String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return;
        }
    };
    runtime.block_on(async {
        let context = match ei::Context::new(socket) {
            Ok(context) => context,
            Err(error) => {
                let _ = ready.send(Err(error.to_string()));
                return;
            }
        };
        let (connection, mut events) = match context
            .handshake_tokio("farsight-agent", ei::handshake::ContextType::Sender)
            .await
        {
            Ok(pair) => pair,
            Err(error) => {
                let _ = ready.send(Err(error.to_string()));
                return;
            }
        };
        if ready.send(Ok(connection.clone())).is_err() {
            return;
        }
        while let Some(event) = events.next().await {
            match event {
                Ok(event) => on_event(state, &connection, event),
                Err(error) => {
                    tracing::debug!("libei connection error: {error}");
                    break;
                }
            }
        }
        tracing::debug!("libei connection closed");
        let mut state = lock(state);
        state.devices.clear();
        state.closed = true;
    });
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn on_event(state: &Mutex<State>, connection: &Connection, event: EiEvent) {
    match event {
        EiEvent::SeatAdded(added) => {
            added.seat.bind_capabilities(
                DeviceCapability::PointerAbsolute
                    | DeviceCapability::Button
                    | DeviceCapability::Scroll
                    | DeviceCapability::Keyboard,
            );
        }
        EiEvent::DeviceResumed(resumed) => {
            let mut state = lock(state);
            let sequence = state.sequence;
            state.sequence = state.sequence.wrapping_add(1);
            resumed
                .device
                .device()
                .start_emulating(connection.serial(), sequence);
            // A keyboard brings the current layout (again after a layout switch).
            if let Some(keys) = resumed.device.keymap().and_then(KeyLookup::from_keymap) {
                state.keys = Some(keys);
            }
            state.devices.retain(|d| d != &resumed.device);
            state.devices.push(resumed.device);
        }
        EiEvent::DevicePaused(paused) => lock(state).devices.retain(|d| d != &paused.device),
        EiEvent::DeviceRemoved(removed) => lock(state).devices.retain(|d| d != &removed.device),
        EiEvent::Disconnected(_) => {
            let mut state = lock(state);
            state.devices.clear();
            state.closed = true;
        }
        _ => {}
    }
    let _ = connection.flush();
}

impl Eis {
    /// Microseconds since we connected, for event timestamps.
    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn finish(&self, device: &Device) {
        device.device().frame(self.connection.serial(), self.now());
        let _ = self.connection.flush();
    }

    /// The compositor ended remote input (input can't reach the desktop any more).
    pub fn closed(&self) -> bool {
        lock(&self.state).closed
    }

    /// Moves the pointer to the fractions `(fx, fy)` (0–1) of the shared screen.
    pub fn move_to(&self, fx: f32, fy: f32) {
        let Some((device, pointer)) = lock(&self.state).with::<ei::PointerAbsolute>() else {
            return;
        };
        let Some(region) = shared_region(device.regions(), self.mapping_id.as_deref()) else {
            tracing::debug!("the libei pointer has no screen region; motion dropped");
            return;
        };
        let (x, y) = place(region, fx, fy);
        pointer.motion_absolute(x, y);
        self.finish(&device);
    }

    /// Presses or releases the evdev button `code`.
    pub fn button(&self, code: i32, pressed: bool) {
        let Some((device, button)) = lock(&self.state).with::<ei::Button>() else {
            return;
        };
        let Ok(code) = u32::try_from(code) else {
            return;
        };
        let state = if pressed {
            ei::button::ButtonState::Press
        } else {
            ei::button::ButtonState::Released
        };
        button.button(code, state);
        self.finish(&device);
    }

    /// One wheel step (`steps` is ±1).
    pub fn scroll(&self, axis: ScrollAxis, steps: i32) {
        let Some((device, scroll)) = lock(&self.state).with::<ei::Scroll>() else {
            return;
        };
        let amount = steps.saturating_mul(WHEEL_STEP);
        match axis {
            ScrollAxis::Vertical => scroll.scroll_discrete(0, amount),
            ScrollAxis::Horizontal => scroll.scroll_discrete(amount, 0),
        }
        self.finish(&device);
    }

    /// Presses or releases the key with evdev code `evdev`.
    pub fn key_code(&self, evdev: u32, pressed: bool) {
        let Some((device, keyboard)) = lock(&self.state).with::<ei::Keyboard>() else {
            return;
        };
        keyboard.key(evdev, key_state(pressed));
        self.finish(&device);
    }

    /// Presses or releases the key that types `keysym` on the compositor's
    /// keymap. Shift and AltGr are added as the symbol needs (held until the
    /// key is released, so autorepeat keeps the right symbol); `held` are
    /// the modifier keysyms the viewer itself is holding.
    pub fn key_sym(&self, keysym: u32, pressed: bool, held: &HashSet<u32>) {
        let (device, keyboard, position, modifier_keys) = {
            let state = lock(&self.state);
            let Some((device, keyboard)) = state.with::<ei::Keyboard>() else {
                return;
            };
            let Some(keys) = state.keys.as_ref() else {
                return;
            };
            let Some(position) = keys.find(keysym) else {
                tracing::debug!("no key on this keymap types the requested symbol");
                return;
            };
            (device, keyboard, position, keys.modifier_keys())
        };
        let mut synthetic = self
            .synthetic
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if pressed {
            let mut added = Vec::new();
            let holds = |keysyms: &[u32]| keysyms.iter().any(|k| held.contains(k));
            if position.shift && !holds(&SHIFT_KEYSYMS) {
                added.push(LEFT_SHIFT);
            }
            if position.altgr && !holds(&ALTGR_KEYSYMS) {
                added.extend(modifier_keys.altgr);
            }
            for &key in &added {
                keyboard.key(key, key_state(true));
            }
            keyboard.key(position.evdev, key_state(true));
            if !added.is_empty() {
                synthetic.insert(keysym, added);
            }
        } else {
            keyboard.key(position.evdev, key_state(false));
            for key in synthetic
                .remove(&keysym)
                .unwrap_or_default()
                .into_iter()
                .rev()
            {
                keyboard.key(key, key_state(false));
            }
        }
        self.finish(&device);
    }
}

/// Whether a keysym is one of the modifiers [`Eis::key_sym`] tracks.
pub(super) fn is_modifier(keysym: u32) -> bool {
    SHIFT_KEYSYMS.contains(&keysym) || ALTGR_KEYSYMS.contains(&keysym)
}

impl Drop for Eis {
    fn drop(&mut self) {
        self.connection.connection().disconnect();
        let _ = self.connection.flush();
        // Ends the thread's read loop (it holds its own handle to the socket).
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

fn key_state(pressed: bool) -> ei::keyboard::KeyState {
    if pressed {
        ei::keyboard::KeyState::Press
    } else {
        ei::keyboard::KeyState::Released
    }
}

/// The region showing the shared stream: the one with its mapping id, else
/// the only region; `None` when that can't be told (motion is then dropped:
/// libei coordinates are logical pixels, a guess could hit the wrong screen).
fn shared_region<'a>(regions: &'a [Region], mapping_id: Option<&str>) -> Option<&'a Region> {
    let only = match regions {
        [only] => Some(only),
        _ => None,
    };
    mapping_id
        .and_then(|id| regions.iter().find(|r| r.mapping_id.as_deref() == Some(id)))
        .or(only)
}

/// The point at fractions `(fx, fy)` of `region` (logical pixels, offset by
/// its position).
fn place(region: &Region, fx: f32, fy: f32) -> (f32, f32) {
    let (fx, fy) = (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0));
    (
        coord(region.x) + fx * coord(region.width.saturating_sub(1)),
        coord(region.y) + fy * coord(region.height.saturating_sub(1)),
    )
}

/// A screen coordinate as `f32` (they fit in 16 bits; larger ones clamp).
fn coord(value: u32) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Where a keysym is on the compositor's keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyPosition {
    /// evdev code.
    evdev: u32,
    /// Typed with Shift (levels 2 and 4).
    shift: bool,
    /// Typed with AltGr (levels 3 and 4).
    altgr: bool,
}

/// The keys that select levels on this keymap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModifierKeys {
    /// AltGr's evdev code, if the keymap has one.
    altgr: Option<u32>,
}

/// Keysym → key, from the keymap the compositor sent.
struct KeyLookup {
    keys: HashMap<u32, KeyPosition>,
}

impl KeyLookup {
    fn from_keymap(keymap: &reis::event::Keymap) -> Option<Self> {
        if keymap.size == 0 || keymap.size > MAX_KEYMAP {
            return None;
        }
        let file = std::fs::File::from(keymap.fd.try_clone().ok()?);
        let mut bytes = vec![0; usize::try_from(keymap.size).ok()?];
        std::os::unix::fs::FileExt::read_exact_at(&file, &mut bytes, 0).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        Self::from_text(text.trim_end_matches('\0'))
    }

    fn from_text(text: &str) -> Option<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            text.to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        let mut keys = HashMap::new();
        keymap.key_for_each(|keymap, keycode| {
            // xkb keycodes are evdev codes plus 8.
            let Some(evdev) = keycode.raw().checked_sub(8) else {
                return;
            };
            // Levels 1–4: plain, Shift, AltGr, Shift+AltGr.
            for level in 0..keymap.num_levels_for_key(keycode, 0).min(4) {
                for sym in keymap.key_get_syms_by_level(keycode, 0, level) {
                    // The first (lowest) position wins, e.g. the main row over the keypad.
                    keys.entry(sym.raw()).or_insert(KeyPosition {
                        evdev,
                        shift: level % 2 == 1,
                        altgr: level >= 2,
                    });
                }
            }
        });
        Some(Self { keys })
    }

    fn find(&self, keysym: u32) -> Option<KeyPosition> {
        self.keys.get(&keysym).copied()
    }

    fn modifier_keys(&self) -> ModifierKeys {
        ModifierKeys {
            altgr: ALTGR_KEYSYMS
                .iter()
                .find_map(|&k| self.find(k))
                .map(|p| p.evdev),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reis::PendingRequestResult;
    use reis::eis;
    use reis::request::{DeviceCapability as Cap, EisRequest, EisRequestConverter};

    /// A minimal libei server (what a compositor runs): one seat; on bind,
    /// an absolute pointer over two monitors (the shared one on the right,
    /// mapping id "shared") and a keyboard, resumed. Reports what it receives
    /// and stops after `rounds` polls (closing the connection).
    fn serve(stream: UnixStream, received: &std::sync::mpsc::Sender<String>, rounds: usize) {
        let context = eis::Context::new(stream).unwrap();
        let mut handshaker = reis::handshake::EisHandshaker::new(&context, 1);
        let mut converter: Option<EisRequestConverter> = None;
        let mut devices = Vec::new();
        let mut seats = Vec::new();
        for _ in 0..rounds {
            if context.read().is_err() {
                return;
            }
            while let Some(result) = context.pending_request() {
                let PendingRequestResult::Request(request) = result else {
                    continue;
                };
                match &mut converter {
                    None => {
                        if let Some(resp) = handshaker.handle_request(request).unwrap() {
                            let c = EisRequestConverter::new(&context, resp, 1);
                            seats.push(c.handle().add_seat(
                                Some("seat"),
                                Cap::PointerAbsolute | Cap::Button | Cap::Scroll | Cap::Keyboard,
                            ));
                            converter = Some(c);
                        }
                    }
                    Some(c) => {
                        c.handle_request(request).unwrap();
                        while let Some(request) = c.next_request() {
                            report(&request, received);
                            if let EisRequest::Bind(bind) = request {
                                devices.push(bind.seat.add_device(
                                    Some("pointer"),
                                    eis::device::DeviceType::Virtual,
                                    Cap::PointerAbsolute | Cap::Button | Cap::Scroll,
                                    |device| {
                                        let d = device.device();
                                        d.region_mapping_id("left");
                                        d.region(0, 0, 1920, 1080, 1.0);
                                        d.region_mapping_id("shared");
                                        d.region(1920, 0, 2560, 1440, 1.0);
                                    },
                                ));
                                devices.push(bind.seat.add_device(
                                    Some("keyboard"),
                                    eis::device::DeviceType::Virtual,
                                    Cap::Keyboard.into(),
                                    |_| {},
                                ));
                                for device in &devices {
                                    device.resumed();
                                }
                            }
                        }
                    }
                }
            }
            let _ = context.flush();
            if let Some(c) = &converter {
                let _ = c.handle().flush();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn report(request: &EisRequest, received: &std::sync::mpsc::Sender<String>) {
        let line = match request {
            EisRequest::PointerMotionAbsolute(m) => {
                format!("move {} {}", m.dx_absolute, m.dy_absolute)
            }
            EisRequest::Button(b) => format!("button {} {:?}", b.button, b.state),
            EisRequest::KeyboardKey(k) => format!("key {} {:?}", k.key, k.state),
            _ => return,
        };
        let _ = received.send(line);
    }

    async fn connected(rounds: usize) -> (Eis, std::sync::mpsc::Receiver<String>) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || serve(theirs, &sent, rounds));
        let eis = connect(OwnedFd::from(ours), Some("shared".into()))
            .await
            .unwrap();
        // Wait for the compositor (here: the test server) to resume both devices.
        for _ in 0..200 {
            if lock(&eis.state).devices.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        (eis, received)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn input_reaches_the_shared_screen_through_libei() {
        let (eis, received) = connected(500).await;
        eis.move_to(0.5, 0.25);
        eis.button(0x110, true);
        eis.key_code(30, true);
        let got: Vec<String> = (0..3)
            .map(|_| received.recv_timeout(Duration::from_secs(3)).unwrap())
            .collect();
        // Half-way across the right-hand (shared) monitor, not the left one.
        assert_eq!(
            got,
            ["move 3199.5 359.75", "button 272 Press", "key 30 Press"]
        );
        assert!(!eis.closed());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_connection_is_noticed() {
        let (eis, _received) = connected(40).await;
        for _ in 0..300 {
            if eis.closed() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the closed libei connection went unnoticed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_never_answers_times_out() {
        let (ours, _theirs) = UnixStream::pair().unwrap();
        let started = std::time::Instant::now();
        assert!(connect(OwnedFd::from(ours), None).await.is_err());
        assert!(started.elapsed() < HANDSHAKE_TIMEOUT + Duration::from_secs(2));
    }

    #[test]
    fn regions_are_matched_by_mapping_id() {
        let region = |x, id: &str| Region {
            x,
            y: 0,
            width: 100,
            height: 100,
            scale: 1.0,
            mapping_id: Some(id.into()),
        };
        let both = [region(0, "a"), region(100, "b")];
        assert_eq!(shared_region(&both, Some("b")).map(|r| r.x), Some(100));
        assert!(
            shared_region(&both, None).is_none(),
            "ambiguous: no guessing"
        );
        assert_eq!(shared_region(&both[..1], None).map(|r| r.x), Some(0));
        assert_eq!(place(&both[1], 2.0, -1.0), (199.0, 0.0));
    }

    fn keymap(layout: &str) -> Option<KeyLookup> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        KeyLookup::from_text(&keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
    }

    #[test]
    fn keysyms_are_found_with_their_modifiers() {
        let Some(us) = keymap("us") else {
            eprintln!("skipping: no xkb keymap data installed");
            return;
        };
        // 'a' is KEY_A (30) plain; 'A' is the same key with Shift.
        assert_eq!(
            us.find(0x61),
            Some(KeyPosition {
                evdev: 30,
                shift: false,
                altgr: false
            })
        );
        assert_eq!(
            us.find(0x41),
            Some(KeyPosition {
                evdev: 30,
                shift: true,
                altgr: false
            })
        );
        assert_eq!(
            us.find(0xff0d).map(|p| p.evdev),
            Some(28),
            "Return is KEY_ENTER"
        );
        // German: '@' is AltGr+Q (KEY_Q = 16), and the keymap has an AltGr key.
        let de = keymap("de").unwrap();
        assert_eq!(
            de.find(0x40),
            Some(KeyPosition {
                evdev: 16,
                shift: false,
                altgr: true
            })
        );
        assert!(de.modifier_keys().altgr.is_some());
        assert!(is_modifier(0xffe1) && is_modifier(0xfe03) && !is_modifier(0x61));
    }
}
