//! Q2: a real headless desktop with a real X11 app (through Xwayland):
//! the app's window is rendered, input reaches it, and the desktop resizes.
//!
//! Needs Xwayland 23.1+ and `XDG_RUNTIME_DIR`; skipped (with a message) without them.
//! CI runs it on Ubuntu 24.04.
#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use farsight_headless::{Frame, HeadlessEvent, Options, ResizeResult, start};
use tokio::sync::mpsc::Receiver;
use tokio::time::timeout;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, EventMask, WindowClass};
use x11rb::rust_connection::RustConnection;

const RED: u32 = 0x00ff_0000;
const WAIT: Duration = Duration::from_secs(15);

fn prerequisites_missing() -> bool {
    let xwayland = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("Xwayland").is_file()));
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").is_some();
    let ok = xwayland && runtime_dir && farsight_headless::xwayland_supported();
    if !ok {
        eprintln!("skipping: needs Xwayland 23.1+ on PATH and XDG_RUNTIME_DIR");
    }
    !ok
}

async fn next_frame(events: &mut Receiver<HeadlessEvent>, ok: impl Fn(&Frame) -> bool) -> Frame {
    timeout(WAIT, async {
        loop {
            match events.recv().await {
                Some(HeadlessEvent::Frame(frame)) if ok(&frame) => return frame,
                Some(HeadlessEvent::Frame(frame)) => eprintln!(
                    "frame {}x{} damage {:?} centre {:06x?}",
                    frame.framebuffer.width(),
                    frame.framebuffer.height(),
                    frame.damage,
                    frame.framebuffer.get(82, 72)
                ),
                Some(HeadlessEvent::Closed(reason)) => panic!("compositor closed: {reason}"),
                Some(_) => {}
                None => panic!("compositor gone"),
            }
        }
    })
    .await
    .expect("timed out waiting for a frame")
}

fn wait_for_x_event(conn: &RustConnection, wanted: impl Fn(&Event) -> bool) -> Event {
    let deadline = std::time::Instant::now() + WAIT;
    while std::time::Instant::now() < deadline {
        if let Some(event) = conn.poll_for_event().unwrap() {
            if wanted(&event) {
                return event;
            }
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    panic!("timed out waiting for an X event");
}

/// Opens a red 100x80 X11 window that listens for clicks and keys.
fn open_red_window(x_display: &str) -> RustConnection {
    let (conn, screen_num) = match x11rb::connect(Some(x_display)) {
        Ok(c) => c,
        Err(x11rb::errors::ConnectError::SetupFailed(f)) => {
            panic!("setup failed: {}", String::from_utf8_lossy(&f.reason))
        }
        Err(e) => panic!("{e}"),
    };
    let screen = &conn.setup().roots[screen_num];
    let window = conn.generate_id().unwrap();
    conn.create_window(
        screen.root_depth,
        window,
        screen.root,
        0,
        0,
        100,
        80,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(RED)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::KEY_PRESS | EventMask::EXPOSURE),
    )
    .unwrap();
    conn.map_window(window).unwrap();
    conn.flush().unwrap();

    conn
}

#[tokio::test(flavor = "multi_thread")]
async fn an_x11_window_is_rendered_gets_input_and_the_desktop_resizes() {
    if prerequisites_missing() {
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let started = start(Options {
        width: 400,
        height: 300,
        ..Options::default()
    })
    .unwrap();
    let mut events = started.events;
    let handle = started.handle;

    let x_display = timeout(WAIT, async {
        loop {
            match events.recv().await {
                Some(HeadlessEvent::Ready { x_display, .. }) => return x_display,
                Some(_) => {}
                None => panic!("compositor gone"),
            }
        }
    })
    .await
    .unwrap()
    .expect("Xwayland started");

    let conn = open_red_window(&x_display);

    // New windows are cascaded from (32, 32).
    let (cx, cy) = (32 + 50, 32 + 40);
    let shown = next_frame(&mut events, |f| f.framebuffer.get(cx, cy) == Some(RED)).await;
    assert_eq!(
        (shown.framebuffer.width(), shown.framebuffer.height()),
        (400, 300)
    );
    assert_ne!(
        shown.framebuffer.get(5, 5),
        Some(RED),
        "the background is not the window"
    );

    // A click inside the window reaches the app...
    handle.pointer(0, cx, cy);
    handle.pointer(1, cx, cy);
    handle.pointer(0, cx, cy);
    let click = wait_for_x_event(&conn, |e| matches!(e, Event::ButtonPress(_)));
    assert!(matches!(click, Event::ButtonPress(b) if b.detail == 1));

    // ...and so does typing ("a" is evdev 30, X keycode 38).
    handle.key(true, u32::from(b'a'));
    handle.key(false, u32::from(b'a'));
    let key = wait_for_x_event(&conn, |e| matches!(e, Event::KeyPress(_)));
    assert!(matches!(key, Event::KeyPress(k) if k.detail == 38));

    // Resizing gives a bigger desktop with the window still there.
    let resize = tokio::task::spawn_blocking({
        let handle = handle.clone();
        move || handle.resize(640, 480, &[])
    });
    assert!(matches!(
        resize.await.unwrap(),
        ResizeResult::Resized { .. }
    ));
    next_frame(&mut events, |f| {
        f.framebuffer.width() == 640 && f.framebuffer.get(cx, cy) == Some(RED)
    })
    .await;

    // A layout that doesn't fit is refused.
    let refused = tokio::task::spawn_blocking({
        let handle = handle.clone();
        move || {
            handle.resize(
                640,
                480,
                &[farsight_rfb::Screen {
                    id: 1,
                    x: 600,
                    y: 0,
                    width: 100,
                    height: 100,
                    flags: 0,
                }],
            )
        }
    });
    assert_eq!(refused.await.unwrap(), ResizeResult::InvalidLayout);
    handle.stop();
}

/// The indicator colour (sRGB 0.86/0.15/0.15) as rendered by pixman.
fn is_indicator(pixel: Option<u32>) -> bool {
    pixel.is_some_and(|p| (p >> 16) & 0xff > 0xc0 && (p >> 8) & 0xff < 0x60 && p & 0xff < 0x60)
}

#[tokio::test(flavor = "multi_thread")]
async fn others_watching_shows_a_frame_and_monitor_layouts_apply() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: needs XDG_RUNTIME_DIR");
        return;
    }
    let started = start(Options {
        width: 320,
        height: 200,
        xwayland: false,
        ..Options::default()
    })
    .unwrap();
    let mut events = started.events;
    let handle = started.handle;

    handle.set_viewers(2);
    next_frame(&mut events, |f| is_indicator(f.framebuffer.get(1, 1))).await;
    handle.set_viewers(1);
    next_frame(&mut events, |f| !is_indicator(f.framebuffer.get(1, 1))).await;

    // Two monitors side by side; with the frame on, each gets its own.
    let screen = |id, x, width| farsight_rfb::Screen {
        id,
        x,
        y: 0,
        width,
        height: 200,
        flags: 0,
    };
    let layout = [screen(1, 0, 320), screen(2, 320, 200)];
    let resized = tokio::task::spawn_blocking({
        let handle = handle.clone();
        move || handle.resize(520, 200, &layout)
    });
    assert!(matches!(
        resized.await.unwrap(),
        ResizeResult::Resized { .. }
    ));
    handle.set_viewers(3);
    next_frame(&mut events, |f| {
        f.framebuffer.width() == 520
            && is_indicator(f.framebuffer.get(318, 100))
            && is_indicator(f.framebuffer.get(321, 100))
            && !is_indicator(f.framebuffer.get(160, 100))
    })
    .await;
    handle.stop();
    let closed = timeout(WAIT, async {
        loop {
            if let Some(HeadlessEvent::Closed(_)) | None = events.recv().await {
                return;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "stopping closes the desktop");
}
