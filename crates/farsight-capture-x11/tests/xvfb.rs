//! Integration tests against a private Xvfb server. Each test starts its own
//! `Xvfb -screen 0 640x480x24` and is skipped (with a message) when Xvfb is
//! not installed. Run them in `scripts/docker-cargo.sh test -p farsight-capture-x11`.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use farsight_capture_x11::{CaptureEvent, Frame, Options, Started, start};
use farsight_rfb::Rect;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::randr::{ConnectionExt as _, Rotation};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, InputFocus,
    KeyButMask, PropMode, Rectangle, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A private Xvfb server, killed on drop.
struct Xvfb {
    child: Child,
    _stdout: BufReader<ChildStdout>,
    display: String,
}

impl Xvfb {
    fn start() -> Option<Self> {
        Self::start_with("640x480x24", &[])
    }

    fn start_with(screen: &str, extra: &[&str]) -> Option<Self> {
        let spawned = Command::new("Xvfb")
            .args(["-displayfd", "1", "-screen", "0", screen])
            .args(["-nolisten", "tcp"])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping: Xvfb is not installed");
                return None;
            }
            Err(err) => panic!("cannot start Xvfb: {err}"),
        };
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let number: u32 = line.trim().parse().expect("Xvfb display number");
        Some(Self {
            child,
            _stdout: stdout,
            display: format!(":{number}"),
        })
    }

    fn connect(&self) -> (RustConnection, Window) {
        let (conn, screen) = x11rb::connect(Some(&self.display)).unwrap();
        let root = conn.setup().roots[screen].root;
        (conn, root)
    }

    fn start_backend(&self) -> Started {
        start(Options {
            display: Some(self.display.clone()),
            ..Options::default()
        })
        .unwrap()
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn next_event(started: &mut Started, deadline: Instant) -> CaptureEvent {
    let left = deadline.saturating_duration_since(Instant::now());
    tokio::time::timeout(left, started.events.recv())
        .await
        .expect("timed out waiting for a capture event")
        .expect("event channel closed")
}

async fn wait_frame(started: &mut Started, mut accept: impl FnMut(&Frame) -> bool) -> Frame {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let CaptureEvent::Frame(frame) = next_event(started, deadline).await
            && accept(&frame)
        {
            return frame;
        }
    }
}

fn poll_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && y >= rect.y && u32::from(x) < rect.right() && u32::from(y) < rect.bottom()
}

/// A mapped 640×480 window at the origin that receives input events.
fn input_window(conn: &RustConnection, root: Window) -> Window {
    let window = conn.generate_id().unwrap();
    let mask = EventMask::KEY_PRESS
        | EventMask::KEY_RELEASE
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::EXPOSURE
        | EventMask::PROPERTY_CHANGE;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        640,
        480,
        0,
        WindowClass::INPUT_OUTPUT,
        COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(mask).override_redirect(1),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.flush().unwrap();
    wait_x_event(conn, |e| matches!(e, Event::Expose(_)));
    window
}

fn wait_x_event(conn: &RustConnection, mut accept: impl FnMut(&Event) -> bool) -> Event {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(event) = conn.poll_for_event().unwrap() {
            if accept(&event) {
                return event;
            }
        } else {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for an X event"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn keycode_of(conn: &RustConnection, keysym: u32) -> Option<u8> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .unwrap()
        .reply()
        .unwrap();
    let per = usize::from(map.keysyms_per_keycode);
    map.keysyms
        .chunks(per)
        .position(|syms| syms.first() == Some(&keysym))
        .map(|i| min + i as u8)
}

fn atom(conn: &RustConnection, name: &str) -> Atom {
    conn.intern_atom(false, name.as_bytes())
        .unwrap()
        .reply()
        .unwrap()
        .atom
}

#[tokio::test]
async fn drawing_produces_damaged_frame_with_pixels() {
    let Some(xvfb) = Xvfb::start() else { return };
    let mut started = xvfb.start_backend();
    let first = wait_frame(&mut started, |f| f.damage.is_none()).await;
    assert_eq!(
        (first.framebuffer.width(), first.framebuffer.height()),
        (640, 480)
    );

    let (conn, root) = xvfb.connect();
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, root, &CreateGCAux::new().foreground(0x00FF_0000))
        .unwrap();
    let target = Rect::new(100, 50, 40, 30);
    conn.poly_fill_rectangle(
        root,
        gc,
        &[Rectangle {
            x: 100,
            y: 50,
            width: 40,
            height: 30,
        }],
    )
    .unwrap();
    conn.flush().unwrap();

    let mut damaged: Vec<Rect> = Vec::new();
    let frame = wait_frame(&mut started, |f| {
        let Some(damage) = &f.damage else {
            return false;
        };
        damaged.extend(damage);
        let covered = (target.y..target.y + target.height).all(|y| {
            (target.x..target.x + target.width).all(|x| damaged.iter().any(|r| contains(*r, x, y)))
        });
        covered && f.framebuffer.get(139, 79) == Some(0x00FF_0000)
    })
    .await;
    let fb = &frame.framebuffer;
    assert_eq!(fb.get(100, 50), Some(0x00FF_0000));
    assert_eq!(fb.get(139, 79), Some(0x00FF_0000));
    assert_ne!(fb.get(140, 80), Some(0x00FF_0000));
    assert_ne!(fb.get(99, 49), Some(0x00FF_0000));
    // Damage is local, not the whole screen.
    assert!(damaged.iter().all(|r| r.area() < 640 * 480));
}

/// Without XDamage and MIT-SHM the backend polls with `GetImage` and
/// reports the changed 32×32 tiles.
#[tokio::test]
async fn polling_without_damage_reports_changed_tiles() {
    let no_damage_no_shm = ["-extension", "DAMAGE", "-extension", "MIT-SHM"];
    let Some(xvfb) = Xvfb::start_with("640x480x24", &no_damage_no_shm) else {
        return;
    };
    let mut started = xvfb.start_backend();
    wait_frame(&mut started, |f| f.damage.is_none()).await;

    let (conn, root) = xvfb.connect();
    let gc = conn.generate_id().unwrap();
    conn.create_gc(gc, root, &CreateGCAux::new().foreground(0x0000_FF00))
        .unwrap();
    let square = Rectangle {
        x: 70,
        y: 70,
        width: 10,
        height: 10,
    };
    conn.poly_fill_rectangle(root, gc, &[square]).unwrap();
    conn.flush().unwrap();

    let frame = wait_frame(&mut started, |f| {
        f.framebuffer.get(75, 75) == Some(0x0000_FF00)
    })
    .await;
    let damage = frame.damage.expect("a polled change carries damage");
    assert!(
        damage
            .iter()
            .any(|r| contains(*r, 70, 70) && contains(*r, 79, 79))
    );
    assert!(damage.iter().all(|r| r.x % 32 == 0 && r.y % 32 == 0));
    assert!(damage.iter().all(|r| r.area() < 640 * 480));
}

#[tokio::test]
async fn cursor_shape_is_reported() {
    let Some(xvfb) = Xvfb::start() else { return };
    let mut started = xvfb.start_backend();
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let CaptureEvent::Cursor(Some(shape)) = next_event(&mut started, deadline).await {
            assert!(shape.width > 0 && shape.height > 0);
            assert_eq!(
                shape.rgba.len(),
                usize::from(shape.width) * usize::from(shape.height) * 4
            );
            return;
        }
    }
}

#[tokio::test]
async fn pointer_motion_and_buttons_are_injected() {
    let Some(xvfb) = Xvfb::start() else { return };
    let started = xvfb.start_backend();
    let (conn, root) = xvfb.connect();
    let window = input_window(&conn, root);
    let input = started.input.clone();

    input.pointer(0, 200, 150);
    poll_until("pointer motion", || {
        let p = conn.query_pointer(root).unwrap().reply().unwrap();
        (p.root_x, p.root_y) == (200, 150)
    });

    input.pointer(1, 200, 150);
    poll_until("button 1 down", || {
        let p = conn.query_pointer(root).unwrap().reply().unwrap();
        u16::from(p.mask) & u16::from(KeyButMask::BUTTON1) != 0
    });
    input.pointer(0b100, 210, 160);
    poll_until("button 1 up, button 3 down", || {
        let p = conn.query_pointer(root).unwrap().reply().unwrap();
        let mask = u16::from(p.mask);
        mask & u16::from(KeyButMask::BUTTON1) == 0
            && mask & u16::from(KeyButMask::BUTTON3) != 0
            && (p.root_x, p.root_y) == (210, 160)
    });
    input.pointer(0, 210, 160);

    // Wheel down (bit 4) and back (bit 7) as presses seen by the window.
    for (bit, button) in [(4u8, 5u8), (7, 8)] {
        input.pointer(1 << bit, 210, 160);
        input.pointer(0, 210, 160);
        let event = wait_x_event(
            &conn,
            |e| matches!(e, Event::ButtonPress(b) if b.event == window && b.detail == button),
        );
        let Event::ButtonPress(press) = event else {
            unreachable!()
        };
        assert_eq!((press.event_x, press.event_y), (210, 160));
    }
    drop(started);
}

#[tokio::test]
async fn key_presses_reach_a_window() {
    let Some(xvfb) = Xvfb::start() else { return };
    let started = xvfb.start_backend();
    let (conn, root) = xvfb.connect();
    let window = input_window(&conn, root);
    conn.set_input_focus(InputFocus::POINTER_ROOT, window, CURRENT_TIME)
        .unwrap();
    conn.flush().unwrap();
    let input = started.input.clone();
    let keycode_a = keycode_of(&conn, 0x61).expect("'a' is mapped");

    input.key(true, 0x61);
    input.key(false, 0x61);
    let event = wait_x_event(&conn, |e| matches!(e, Event::KeyPress(_)));
    let Event::KeyPress(press) = event else {
        unreachable!()
    };
    assert_eq!(press.detail, keycode_a);
    assert_eq!(u16::from(press.state) & u16::from(KeyButMask::SHIFT), 0);
    wait_x_event(
        &conn,
        |e| matches!(e, Event::KeyRelease(r) if r.detail == keycode_a),
    );

    // 'A' needs Shift, which is pressed for it.
    input.key(true, 0x41);
    input.key(false, 0x41);
    let event = wait_x_event(
        &conn,
        |e| matches!(e, Event::KeyPress(p) if p.detail == keycode_a),
    );
    let Event::KeyPress(press) = event else {
        unreachable!()
    };
    assert_ne!(u16::from(press.state) & u16::from(KeyButMask::SHIFT), 0);
    wait_x_event(
        &conn,
        |e| matches!(e, Event::KeyRelease(r) if r.detail == keycode_a),
    );
    let shift_down = conn.query_pointer(root).unwrap().reply().unwrap().mask;
    assert_eq!(u16::from(shift_down) & u16::from(KeyButMask::SHIFT), 0);

    // A keysym with no key (U+263A) goes through a temporarily remapped keycode.
    let smiley = 0x0100_263A;
    assert_eq!(keycode_of(&conn, smiley), None);
    input.key(true, smiley);
    let event = wait_x_event(&conn, |e| matches!(e, Event::KeyPress(_)));
    let Event::KeyPress(press) = event else {
        unreachable!()
    };
    assert_ne!(press.detail, keycode_a);
    input.key(false, smiley);
    wait_x_event(
        &conn,
        |e| matches!(e, Event::KeyRelease(r) if r.detail == press.detail),
    );
    poll_until("remapped keycode restored", || {
        keycode_of(&conn, smiley).is_none()
    });
    drop(started);
}

#[tokio::test]
async fn clipboard_from_x_is_reported() {
    let Some(xvfb) = Xvfb::start() else { return };
    let mut started = xvfb.start_backend();
    let (conn, root) = xvfb.connect();
    let window = input_window(&conn, root);
    let clipboard = atom(&conn, "CLIPBOARD");
    let utf8 = atom(&conn, "UTF8_STRING");
    conn.set_selection_owner(window, clipboard, CURRENT_TIME)
        .unwrap();
    conn.flush().unwrap();

    // Serve requests on a helper thread until the server goes away.
    std::thread::spawn(move || {
        while let Ok(event) = conn.wait_for_event() {
            let Event::SelectionRequest(req) = event else {
                continue;
            };
            let served = req.target == utf8;
            if served {
                conn.change_property8(
                    PropMode::REPLACE,
                    req.requestor,
                    req.property,
                    utf8,
                    "hello from X ✓".as_bytes(),
                )
                .unwrap();
            }
            let notify = SelectionNotifyEvent {
                response_type: SELECTION_NOTIFY_EVENT,
                sequence: 0,
                time: req.time,
                requestor: req.requestor,
                selection: req.selection,
                target: req.target,
                property: if served { req.property } else { NONE },
            };
            let _ = conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify);
            let _ = conn.flush();
        }
    });

    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let CaptureEvent::Clipboard(text) = next_event(&mut started, deadline).await {
            assert_eq!(text, "hello from X ✓");
            return;
        }
    }
}

#[tokio::test]
async fn clipboard_from_remote_is_served() {
    let Some(xvfb) = Xvfb::start() else { return };
    let started = xvfb.start_backend();
    let (conn, root) = xvfb.connect();
    let window = input_window(&conn, root);
    let clipboard = atom(&conn, "CLIPBOARD");
    let utf8 = atom(&conn, "UTF8_STRING");
    let targets = atom(&conn, "TARGETS");
    let property = atom(&conn, "TEST_CLIP");

    started.input.set_clipboard("from remote ✓".to_owned());
    poll_until("clipboard owner", || {
        let owner = conn
            .get_selection_owner(clipboard)
            .unwrap()
            .reply()
            .unwrap()
            .owner;
        owner != NONE && owner != window
    });

    let fetch = |target: Atom| {
        conn.convert_selection(window, clipboard, target, property, CURRENT_TIME)
            .unwrap();
        conn.flush().unwrap();
        let event = wait_x_event(&conn, |e| matches!(e, Event::SelectionNotify(_)));
        let Event::SelectionNotify(notify) = event else {
            unreachable!()
        };
        assert_eq!(notify.property, property, "request refused");
        conn.get_property(true, window, property, AtomEnum::ANY, 0, 1 << 20)
            .unwrap()
            .reply()
            .unwrap()
    };

    let text = fetch(utf8);
    assert_eq!(text.type_, utf8);
    assert_eq!(String::from_utf8(text.value).unwrap(), "from remote ✓");

    let offered = fetch(targets);
    let atoms: Vec<u32> = offered.value32().unwrap().collect();
    assert!(atoms.contains(&utf8));
    assert!(atoms.contains(&u32::from(AtomEnum::STRING)));

    let latin = fetch(AtomEnum::STRING.into());
    assert_eq!(latin.value, b"from remote ?");
    drop(started);
}

/// Xvfb cannot grow past its initial size, so this starts at 800×600 and
/// shrinks to 640×480 with RandR 1.2 `SetScreenSize`.
#[tokio::test]
async fn randr_resize_sends_full_frame() {
    let Some(xvfb) = Xvfb::start_with("800x600x24", &[]) else {
        return;
    };
    let mut started = xvfb.start_backend();
    wait_frame(&mut started, |f| f.damage.is_none()).await;
    let (conn, root) = xvfb.connect();
    if !matches!(
        conn.randr_query_version(1, 3)
            .map(x11rb::cookie::Cookie::reply),
        Ok(Ok(_))
    ) {
        eprintln!("skipping: no RandR");
        return;
    }
    // Active CRTCs must fit the new size: switch them off first.
    let resources = conn
        .randr_get_screen_resources_current(root)
        .unwrap()
        .reply()
        .unwrap();
    for crtc in resources.crtcs {
        let _ = conn
            .randr_set_crtc_config(
                crtc,
                CURRENT_TIME,
                resources.config_timestamp,
                0,
                0,
                NONE,
                Rotation::ROTATE0,
                &[],
            )
            .unwrap()
            .reply();
    }
    let resized = conn
        .randr_set_screen_size(root, 640, 480, 169, 127)
        .unwrap()
        .check();
    if let Err(err) = resized {
        eprintln!("skipping: Xvfb refused RandR resize: {err}");
        return;
    }
    let frame = wait_frame(&mut started, |f| {
        f.framebuffer.width() == 640 && f.framebuffer.height() == 480
    })
    .await;
    assert!(frame.damage.is_none(), "a resize sends a full frame");
    assert_eq!(frame.framebuffer.pixels().len(), 640 * 480);
}
