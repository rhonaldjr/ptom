//! The compositor: its state, event loop, input and window placement.

use std::process::Child;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::desktop::{PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::keyboard::{FilterResult, Keycode, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent};
use smithay::input::{Seat, SeatState};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{
    EventLoop, Interest, LoopHandle, LoopSignal, Mode, PostAction, channel,
};
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::xwayland_shell::XWaylandShellState;
use smithay::xwayland::{X11Wm, XWayland, XWaylandEvent};
use tokio::sync::mpsc::Sender;
use tracing::{debug, info, warn};

use crate::focus::KeyboardFocus;
use crate::grab::BTN_LEFT;
use crate::keys::{self, KeyLookup, SHIFT_KEYS};
use crate::render::Screens;
use crate::{Command, HeadlessEvent, Options, ResizeResult, clipboard, layout, session};

/// How long the session's apps get to exit after the desktop stops.
const SESSION_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for Xwayland before starting the session without it.
const XWAYLAND_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the desktop is re-rendered when something changed (~60 fps).
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Linux `BTN_RIGHT` and `BTN_MIDDLE`.
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
/// One wheel notch in Wayland's high-resolution (v120) units, and in pixels.
const WHEEL_V120: i32 = 120;
const WHEEL_STEP: f64 = 15.0;
/// Where the first window goes, and how far each next one is shifted.
const CASCADE_START: i32 = 32;
const CASCADE_STEP: i32 = 32;
const CASCADE_LIMIT: i32 = 10;

/// X servers put their sockets in `/tmp/.X11-unix`, which the system
/// normally creates at boot (owned by root, sticky, world-writable). Create
/// it where it doesn't exist (e.g. containers), and refuse one that another
/// user owns or that lacks the sticky bit: they could swap our socket.
fn ensure_x11_socket_dir() -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let dir = std::path::Path::new("/tmp/.X11-unix");
    if !dir.exists() && std::fs::DirBuilder::new().mode(0o1777).create(dir).is_ok() {
        // The umask may have removed bits; set them explicitly.
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777));
    }
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("/tmp/.X11-unix: {e}"))?;
    let me = rustix::process::geteuid().as_raw();
    let owner_ok = meta.uid() == 0 || meta.uid() == me;
    let others_can_write = meta.mode() & 0o022 != 0;
    let sticky = meta.mode() & 0o1000 != 0;
    if !meta.is_dir() || !owner_ok || (others_can_write && !sticky) {
        return Err(
            "/tmp/.X11-unix is not owned by root or you, or is writable without the sticky bit"
                .into(),
        );
    }
    Ok(())
}

/// Xwayland here accepts any local client that can open its socket (there's
/// no X authority file), so the socket is made owner-only.
fn restrict_x11_socket(display: u32) {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::fs::PermissionsExt;
    let socket = format!("/tmp/.X11-unix/X{display}");
    // Never follow a symlink someone put there.
    let is_socket = std::fs::symlink_metadata(&socket).is_ok_and(|m| m.file_type().is_socket());
    if !is_socket {
        warn!(%socket, "the X11 socket is not a socket; not touching it");
        return;
    }
    if let Err(error) = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o700)) {
        warn!(%error, %socket, "could not restrict the X11 socket to this user");
    }
}

/// Per-client data Smithay needs.
#[derive(Default)]
pub(crate) struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Who put the current clipboard contents there, so requests for it can be
/// answered from the right place.
#[derive(Debug, Clone)]
pub(crate) enum ClipboardOwner {
    /// Text a viewer sent.
    Viewer(Arc<str>),
    /// An X11 app (through Xwayland).
    Xwayland,
}

// Smithay's handler traits expect these `*_state` field names.
#[allow(clippy::struct_field_names)]
pub(crate) struct State {
    pub display_handle: DisplayHandle,
    /// Taken out while dispatching, because dispatching needs `&mut State`.
    display: Option<Display<State>>,
    pub loop_handle: LoopHandle<'static, State>,
    loop_signal: LoopSignal,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    _output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub xwayland_shell_state: XWaylandShellState,
    pub popups: PopupManager,
    pub space: Space<Window>,
    pub seat: Seat<State>,
    pub xwm: Option<X11Wm>,
    pub screens: Screens,
    pub events: Sender<HeadlessEvent>,
    keys: Option<KeyLookup>,
    /// RFB button mask from the last pointer event.
    buttons: u8,
    start_time: Instant,
    windows_placed: i32,
    /// New windows that get the keyboard once they've drawn a frame.
    pub focus_when_drawn: Vec<Window>,
    wayland_display: String,
    x_display: Option<u32>,
    startup: Option<std::path::PathBuf>,
    session_umask: Option<u32>,
    session: Option<Child>,
    /// The session's process group; it outlives the script when the script
    /// starts apps in the background and exits.
    session_group: Option<rustix::process::Pid>,
    ready_sent: bool,
    pub needs_render: bool,
    pub cursor: CursorImageStatus,
    pub cursor_dirty: bool,
}

/// Runs the compositor until it is stopped. Reports startup success or
/// failure on `started` first.
pub(crate) fn run(
    options: &Options,
    events: Sender<HeadlessEvent>,
    commands: channel::Channel<Command>,
    started: &std_mpsc::Sender<Result<(), String>>,
) {
    let (mut event_loop, mut state) = match State::new(options, events, commands) {
        Ok(created) => created,
        Err(reason) => {
            let _ = started.send(Err(reason));
            return;
        }
    };
    let _ = started.send(Ok(()));
    if options.xwayland {
        state.start_xwayland(options.x_display);
    } else {
        state.desktop_ready();
    }
    let reason = match event_loop.run(None, &mut state, |state| {
        let _ = state.display_handle.flush_clients();
    }) {
        Ok(()) => "the desktop was stopped".to_owned(),
        Err(error) => format!("the compositor failed: {error}"),
    };
    info!(%reason, "headless desktop closed");
    state.end_session();
    let _ = state.events.blocking_send(HeadlessEvent::Closed(reason));
}

impl State {
    fn new(
        options: &Options,
        events: Sender<HeadlessEvent>,
        commands: channel::Channel<Command>,
    ) -> Result<(EventLoop<'static, State>, State), String> {
        let event_loop: EventLoop<'static, State> =
            EventLoop::try_new().map_err(|e| format!("event loop: {e}"))?;
        let mut display: Display<State> =
            Display::new().map_err(|e| format!("Wayland display: {e}"))?;
        let dh = display.handle();

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "farsight");
        seat.add_keyboard(XkbConfig::default(), 400, 30)
            .map_err(|e| format!("keyboard: {e}"))?;
        seat.add_pointer();

        let socket =
            ListeningSocketSource::new_auto().map_err(|e| format!("Wayland socket: {e}"))?;
        let wayland_display = socket.socket_name().to_string_lossy().into_owned();
        let handle = event_loop.handle();
        handle
            .insert_source(socket, |stream, (), state: &mut State| {
                if let Err(error) = state
                    .display_handle
                    .insert_client(stream, Arc::new(ClientState::default()))
                {
                    warn!(%error, "could not accept a Wayland client");
                }
            })
            .map_err(|e| format!("Wayland socket source: {e}"))?;
        // Poll a duplicate of the display's fd and dispatch through `State`,
        // so the display itself never has to be borrowed by the event loop.
        let poll_fd = display
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .map_err(|e| format!("display fd: {e}"))?;
        handle
            .insert_source(
                Generic::new(poll_fd, Interest::READ, Mode::Level),
                |_, _, state| {
                    state.dispatch_clients();
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| format!("display source: {e}"))?;
        handle
            .insert_source(commands, |event, (), state| match event {
                channel::Event::Msg(command) => state.handle(command),
                channel::Event::Closed => state.loop_signal.stop(),
            })
            .map_err(|e| format!("command source: {e}"))?;
        handle
            .insert_source(Timer::from_duration(FRAME_INTERVAL), |_, (), state| {
                state.tick();
                TimeoutAction::ToDuration(FRAME_INTERVAL)
            })
            .map_err(|e| format!("frame timer: {e}"))?;

        let mut space = Space::default();
        let layout = layout::validate_layout(options.width, options.height, &[])
            .map_err(|e| e.to_string())?;
        let screens = Screens::new(&dh, &mut space, options.width, options.height, &layout)?;

        let state = State {
            compositor_state: CompositorState::new::<State>(&dh),
            xdg_shell_state: XdgShellState::new::<State>(&dh),
            shm_state: ShmState::new::<State>(&dh, Vec::new()),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<State>(&dh),
            data_device_state: DataDeviceState::new::<State>(&dh),
            xwayland_shell_state: XWaylandShellState::new::<State>(&dh),
            seat_state,
            popups: PopupManager::default(),
            space,
            seat,
            xwm: None,
            screens,
            events,
            keys: KeyLookup::new(),
            buttons: 0,
            start_time: Instant::now(),
            windows_placed: 0,
            focus_when_drawn: Vec::new(),
            wayland_display,
            x_display: None,
            startup: options.startup.clone(),
            session_umask: options.session_umask,
            session: None,
            session_group: None,
            ready_sent: false,
            needs_render: true,
            cursor: CursorImageStatus::default_named(),
            cursor_dirty: true,
            loop_signal: event_loop.get_signal(),
            loop_handle: handle,
            display: Some(display),
            display_handle: dh,
        };
        if state.keys.is_none() {
            warn!("no keyboard layout data (xkeyboard-config); typing will not work");
        }
        info!(wayland_display = %state.wayland_display, "headless desktop started");
        Ok((event_loop, state))
    }

    fn dispatch_clients(&mut self) {
        if let Some(mut display) = self.display.take() {
            if let Err(error) = display.dispatch_clients(self) {
                warn!(%error, "Wayland dispatch failed");
            }
            self.display = Some(display);
        }
    }

    fn start_xwayland(&mut self, display: Option<u32>) {
        if !crate::xwayland_supported() {
            warn!(
                "X11 apps need Xwayland 23.1 or newer (Ubuntu/Pop!_OS 24.04+); starting without them"
            );
            self.desktop_ready();
            return;
        }
        if let Err(reason) = ensure_x11_socket_dir() {
            warn!(%reason, "not starting Xwayland; X11 apps won't work");
            self.desktop_ready();
            return;
        }
        let spawned = XWayland::spawn(
            &self.display_handle,
            display,
            std::iter::empty::<(String, String)>(),
            // Only the socket file in /tmp/.X11-unix, whose permissions keep
            // other users out; abstract sockets have no permissions.
            false,
            std::process::Stdio::null(),
            std::process::Stdio::null(),
            |_| (),
        );
        let (xwayland, client) = match spawned {
            Ok(spawned) => {
                // As early as possible; the agent's umask already keeps it private.
                restrict_x11_socket(spawned.0.display_number());
                spawned
            }
            Err(error) => {
                warn!(%error, "could not start Xwayland; X11 apps won't work");
                self.desktop_ready();
                return;
            }
        };
        let inserted =
            self.loop_handle
                .insert_source(xwayland, move |event, (), state| match event {
                    XWaylandEvent::Ready {
                        x11_socket,
                        display_number,
                    } => {
                        restrict_x11_socket(display_number);
                        match X11Wm::start_wm(state.loop_handle.clone(), x11_socket, client.clone())
                        {
                            Ok(wm) => {
                                state.xwm = Some(wm);
                                state.x_display = Some(display_number);
                            }
                            Err(error) => warn!(%error, "could not manage X11 windows"),
                        }
                        state.desktop_ready();
                    }
                    XWaylandEvent::Error => {
                        warn!("Xwayland failed to start; X11 apps won't work");
                        state.desktop_ready();
                    }
                });
        if let Err(error) = inserted {
            warn!(%error, "could not watch Xwayland");
            self.desktop_ready();
            return;
        }
        // Xwayland can die before saying anything; don't wait forever.
        let watchdog = self.loop_handle.insert_source(
            Timer::from_duration(XWAYLAND_TIMEOUT),
            |_, (), state| {
                if !state.ready_sent {
                    warn!("Xwayland didn't start in time; X11 apps won't work");
                    state.desktop_ready();
                }
                TimeoutAction::Drop
            },
        );
        if watchdog.is_err() {
            self.desktop_ready();
        }
    }

    /// The Wayland socket (and Xwayland, if any) are up: tell the agent and
    /// start the user's session.
    fn desktop_ready(&mut self) {
        if self.ready_sent {
            return;
        }
        self.ready_sent = true;
        let x_display = self.x_display.map(|n| format!(":{n}"));
        let _ = self.events.try_send(HeadlessEvent::Ready {
            wayland_display: self.wayland_display.clone(),
            x_display: x_display.clone(),
        });
        if let Some(script) = self.startup.clone() {
            match session::start(
                &script,
                &self.wayland_display,
                x_display.as_deref(),
                self.session_umask,
            ) {
                Ok(child) => {
                    self.session_group = rustix::process::Pid::from_child(&child).into();
                    self.session = Some(child);
                }
                Err(error) => {
                    warn!(%error, script = %script.display(), "could not run the startup script");
                }
            }
        }
    }

    /// Stops everything the startup script started (its whole process
    /// group, even after the script itself has exited). Anything still
    /// running after a short grace period is killed.
    fn end_session(&mut self) {
        use rustix::process::{Signal, kill_process_group};
        let Some(group) = self.session_group.take() else {
            return;
        };
        let _ = kill_process_group(group, Signal::TERM);
        let deadline = Instant::now() + SESSION_GRACE;
        if let Some(child) = self.session.as_mut() {
            while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        // Any member left (or one that ignores TERM) is killed.
        let _ = kill_process_group(group, Signal::KILL);
        if let Some(mut child) = self.session.take() {
            let _ = child.wait();
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Key { down, keysym } => self.key(down, keysym),
            Command::Pointer { buttons, x, y } => self.pointer(buttons, x, y),
            Command::Clipboard(text) => clipboard::set_from_viewer(self, text),
            Command::ReleaseAll => self.release_all(),
            Command::Viewers(count) => {
                self.screens.set_indicator(count >= 2);
                self.needs_render = true;
            }
            Command::Resize {
                width,
                height,
                screens,
                reply,
            } => {
                let _ = reply.send(self.resize(width, height, &screens));
            }
            Command::Stop => self.loop_signal.stop(),
        }
    }

    fn resize(
        &mut self,
        width: u16,
        height: u16,
        screens: &[farsight_rfb::Screen],
    ) -> ResizeResult {
        let Ok(layout) = layout::validate_layout(width, height, screens) else {
            return ResizeResult::InvalidLayout;
        };
        if let Err(error) = self.screens.reconfigure(
            &self.display_handle,
            &mut self.space,
            width,
            height,
            &layout,
        ) {
            warn!(%error, "could not resize the desktop");
            return ResizeResult::Failed;
        }
        self.keep_windows_visible();
        self.needs_render = true;
        ResizeResult::Resized {
            generation: self.screens.generation(),
        }
    }

    /// After shrinking, pulls windows whose top-left left the desktop back in.
    fn keep_windows_visible(&mut self) {
        let (width, height) = self.screens.size();
        let moves: Vec<_> = self
            .space
            .elements()
            .filter_map(|w| {
                let at = self.space.element_location(w)?;
                let x = at.x.clamp(0, i32::from(width).saturating_sub(64).max(0));
                let y = at.y.clamp(0, i32::from(height).saturating_sub(64).max(0));
                ((x, y) != (at.x, at.y)).then(|| (w.clone(), Point::from((x, y))))
            })
            .collect();
        for (window, at) in moves {
            self.move_window(&window, at);
        }
    }

    /// Lets go of every key and pointer button currently held.
    fn release_all(&mut self) {
        if let Some(keyboard) = self.seat.get_keyboard() {
            let time = self.time();
            for key in keyboard.pressed_keys() {
                keyboard.input::<(), _>(
                    self,
                    key,
                    KeyState::Released,
                    SERIAL_COUNTER.next_serial(),
                    time,
                    |_, _, _| FilterResult::Forward,
                );
            }
        }
        if self.buttons != 0
            && let Some(pointer) = self.seat.get_pointer()
        {
            let location = pointer.current_location();
            let (x, y) = (location.x.max(0.0), location.y.max(0.0));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            self.pointer(
                0,
                x.min(f64::from(u16::MAX)) as u16,
                y.min(f64::from(u16::MAX)) as u16,
            );
        }
    }

    fn time(&self) -> u32 {
        u32::try_from(self.start_time.elapsed().as_millis() % u128::from(u32::MAX)).unwrap_or(0)
    }

    fn key(&mut self, down: bool, keysym: u32) {
        let Some(position) = self.keys.as_ref().and_then(|k| k.find(keysym)) else {
            debug!(keysym, "keysym not on the keyboard; ignored");
            return;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let held: Vec<u32> = keyboard
            .pressed_keys()
            .iter()
            .map(|k| k.raw())
            .filter(|k| SHIFT_KEYS.contains(k))
            .collect();
        // Viewers with other layouts may send "A" without Shift, or "/" with
        // Shift held (their Shift+7); the US keymap needs the opposite.
        let fix = keys::shift_fix(down, position.shifted, &held);
        let time = self.time();
        let press = |state: &mut State, keycode: u32, key_state: KeyState| {
            keyboard.input::<(), _>(
                state,
                Keycode::new(keycode),
                key_state,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        };
        for &(keycode, pressed) in &fix.before {
            press(
                self,
                keycode,
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
            );
        }
        let state = if down {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        press(self, position.keycode, state);
        for &(keycode, pressed) in &fix.after {
            press(
                self,
                keycode,
                if pressed {
                    KeyState::Pressed
                } else {
                    KeyState::Released
                },
            );
        }
    }

    fn pointer(&mut self, buttons: u8, x: u16, y: u16) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let location: Point<f64, Logical> = (f64::from(x), f64::from(y)).into();
        let time = self.time();
        let under = self.surface_under(location);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        let changed = buttons ^ self.buttons;
        let pressed = buttons & changed;
        for (bit, button) in [(0, BTN_LEFT), (1, BTN_MIDDLE), (2, BTN_RIGHT)] {
            if changed & (1 << bit) == 0 {
                continue;
            }
            let down = buttons & (1 << bit) != 0;
            if down && bit == 0 && !pointer.is_grabbed() {
                self.focus_window_at(location);
            }
            pointer.button(
                self,
                &ButtonEvent {
                    button,
                    state: if down {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Released
                    },
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                },
            );
        }
        // Wheel "buttons" are pressed and released for each notch.
        for (bit, axis, direction) in [
            (3, Axis::Vertical, -1),
            (4, Axis::Vertical, 1),
            (5, Axis::Horizontal, -1),
            (6, Axis::Horizontal, 1),
        ] {
            if pressed & (1 << bit) != 0 {
                let frame = AxisFrame::new(time)
                    .source(AxisSource::Wheel)
                    .value(axis, f64::from(direction) * WHEEL_STEP)
                    .v120(axis, direction * WHEEL_V120);
                pointer.axis(self, frame);
            }
        }
        pointer.frame(self);
        self.buttons = buttons;
    }

    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space.element_under(location).and_then(|(window, at)| {
            window
                .surface_under(location - at.to_f64(), WindowSurfaceType::ALL)
                .map(|(surface, offset)| (surface, (offset + at).to_f64()))
        })
    }

    fn focus_window_at(&mut self, location: Point<f64, Logical>) {
        let window = self.space.element_under(location).map(|(w, _)| w.clone());
        self.focus(window.as_ref());
    }

    /// Raises `window` and gives it the keyboard (or clears focus).
    pub fn focus(&mut self, window: Option<&Window>) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let Some(window) = window else {
            keyboard.set_focus(self, None, serial);
            return;
        };
        self.space.raise_element(window, true);
        let target = if let Some(x11) = window.x11_surface() {
            if let Some(xwm) = self.xwm.as_mut() {
                let _ = xwm.raise_window(x11);
            }
            Some(KeyboardFocus::X11(x11.clone()))
        } else {
            window
                .toplevel()
                .map(|toplevel| KeyboardFocus::Wayland(toplevel.wl_surface().clone()))
        };
        keyboard.set_focus(self, target, serial);
        for other in self.space.elements() {
            if let Some(toplevel) = other.toplevel() {
                toplevel.send_pending_configure();
            }
        }
        self.needs_render = true;
    }

    /// Where the next new window goes: cascaded from the top-left.
    pub fn next_window_location(&mut self) -> Point<i32, Logical> {
        let step = self.windows_placed % CASCADE_LIMIT;
        self.windows_placed += 1;
        let offset = CASCADE_START + step * CASCADE_STEP;
        (offset, offset).into()
    }

    pub fn move_window(&mut self, window: &Window, location: Point<i32, Logical>) {
        self.space.map_element(window.clone(), location, true);
        if let Some(x11) = window.x11_surface() {
            let mut geometry = x11.geometry();
            geometry.loc = location;
            let _ = x11.configure(Some(geometry));
        }
        self.needs_render = true;
    }

    /// The window whose surface tree contains `surface`'s root.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|w| w.wl_surface().is_some_and(|s| *s == *surface))
            .cloned()
    }

    fn tick(&mut self) {
        self.reap_session();
        if self.cursor_dirty {
            let shape = self.screens.cursor_shape(&self.cursor);
            // Retried next tick if the agent is behind.
            self.cursor_dirty = self.events.try_send(HeadlessEvent::Cursor(shape)).is_err();
        }
        if !self.needs_render && !self.screens.has_unsent_frame() {
            return;
        }
        // The agent is behind: don't render or copy a frame it can't take.
        // The damage tracker keeps the changes until the next render.
        if self.events.capacity() == 0 {
            return;
        }
        self.needs_render = false;
        self.space.refresh();
        self.popups.cleanup();
        // If the agent is behind, the damage is kept for the next frame.
        if let Some(frame) = self.screens.render(&self.space)
            && self.events.try_send(HeadlessEvent::Frame(frame)).is_ok()
        {
            self.screens.frame_sent();
        }
        let elapsed = self.start_time.elapsed();
        for window in self.space.elements() {
            if let Some(output) = self.space.outputs_for_element(window).first().cloned() {
                window.send_frame(&output, elapsed, Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
        }
    }

    fn reap_session(&mut self) {
        if let Some(child) = self.session.as_mut()
            && let Ok(Some(status)) = child.try_wait()
        {
            info!(%status, "the headless session's startup script finished");
            self.session = None;
        }
    }
}
