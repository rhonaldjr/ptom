//! One interface over the capture backends (ADR-0018).

use std::path::Path;

#[cfg(not(feature = "wayland"))]
use anyhow::bail;
use anyhow::{Context, Result};
use farsight_codec::{Cursor, Framebuffer};
use farsight_rfb::Rect;
use tokio::sync::mpsc;

use crate::options::Backend;

/// Something the desktop backend produced.
pub enum Event {
    /// A frame, its damage, and its layout generation (0 for live desktops).
    Frame(Framebuffer, Option<Vec<Rect>>, u64),
    Cursor(Option<Cursor>),
    Clipboard(String),
    /// Keyboard LEDs: bit 0 Scroll, 1 Num, 2 Caps Lock.
    Leds(u8),
    Closed(String),
}

/// Sends input to the desktop.
#[derive(Clone)]
pub enum Input {
    X11(farsight_capture_x11::InputHandle),
    Headless(farsight_headless::Handle),
    #[cfg(feature = "wayland")]
    Wayland(farsight_capture_wayland::InputHandle),
}

impl Input {
    pub fn key(&self, down: bool, keysym: u32) {
        match self {
            Self::X11(i) => i.key(down, keysym),
            Self::Headless(i) => i.key(down, keysym),
            #[cfg(feature = "wayland")]
            Self::Wayland(i) => i.key(down, keysym),
        }
    }

    /// A physical key (Linux evdev code) from a QEMU key event.
    pub fn key_code(&self, down: bool, keysym: u32, evdev: u16) {
        match self {
            Self::X11(i) => i.key_code(down, keysym, evdev),
            Self::Headless(i) => i.key_code(down, evdev),
            #[cfg(feature = "wayland")]
            Self::Wayland(i) => i.key_code(down, evdev),
        }
    }

    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        match self {
            Self::X11(i) => i.pointer(buttons, x, y),
            Self::Headless(i) => i.pointer(buttons, x, y),
            #[cfg(feature = "wayland")]
            Self::Wayland(i) => i.pointer(buttons, x, y),
        }
    }

    pub fn set_clipboard(&self, text: &str) {
        match self {
            Self::X11(i) => i.set_clipboard(text.to_owned()),
            Self::Headless(i) => i.set_clipboard(text.to_owned()),
            // The portal has no clipboard yet (planned with the Clipboard portal).
            #[cfg(feature = "wayland")]
            Self::Wayland(_) => {}
        }
    }
}

impl Input {
    /// Lets go of every key and button viewers left held, so the next
    /// controller (U-9) or the person at the screen doesn't inherit them.
    pub fn release_all(&self) {
        match self {
            Self::X11(i) => i.release_all(),
            Self::Headless(i) => i.release_all(),
            #[cfg(feature = "wayland")]
            Self::Wayland(i) => i.release_all(),
        }
    }
}

/// A running backend.
pub struct Capture {
    pub events: mpsc::Receiver<Event>,
    pub input: Input,
    /// Headless only: the compositor thread, joined at shutdown.
    pub compositor: Option<std::thread::JoinHandle<()>>,
    /// Whether viewers' input can reach the desktop (not on a Wayland
    /// desktop whose portal only shares the screen).
    pub can_control: bool,
}

/// Stops a headless desktop and waits (briefly) for its session to be cleaned up.
pub async fn shutdown(input: &Input, compositor: Option<std::thread::JoinHandle<()>>) {
    let (Input::Headless(handle), Some(thread)) = (input, compositor) else {
        return;
    };
    handle.stop();
    let joined = tokio::task::spawn_blocking(move || thread.join());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), joined).await;
}

/// What a headless desktop needs (M7).
pub struct HeadlessSetup {
    /// For the X11 backend: the display to open (`None` = `$DISPLAY`).
    pub x11_display: Option<String>,
    /// The umask the session's apps get (the agent itself runs with 077).
    pub session_umask: Option<u32>,
    pub width: u16,
    pub height: u16,
    pub display: Option<u32>,
    pub startup: Option<std::path::PathBuf>,
    /// The agent runs view-only (`--view-only`): live Wayland sharing then
    /// never asks for input control.
    #[cfg_attr(not(feature = "wayland"), allow(dead_code))]
    pub view_only: bool,
}

/// Starts the backend for `backend` (already resolved from `Auto`).
/// `token_file` keeps the Wayland portal approval between runs (U-3).
#[cfg_attr(not(feature = "wayland"), allow(clippy::unused_async))]
pub async fn start(
    backend: Backend,
    token_file: &Path,
    headless: HeadlessSetup,
) -> Result<Capture> {
    match backend {
        Backend::X11 | Backend::Auto => start_x11(headless.x11_display),
        Backend::Headless => start_headless(headless),
        #[cfg(feature = "wayland")]
        Backend::Wayland => start_wayland(token_file, headless.view_only).await,
        #[cfg(not(feature = "wayland"))]
        Backend::Wayland => {
            let _ = token_file;
            bail!("this build has no Wayland support; rebuild with `--features wayland`")
        }
    }
}

#[cfg(feature = "wayland")]
async fn start_wayland(token_file: &Path, view_only: bool) -> Result<Capture> {
    use farsight_capture_wayland::{CaptureEvent, Options, start};
    let restore_token = std::fs::read_to_string(token_file)
        .ok()
        .map(|t| t.trim().to_owned());
    let started = start(Options {
        restore_token,
        persist: true,
        view_only,
    })
    .await
    .context("could not share the Wayland session")?;
    if let Some(token) = &started.restore_token
        && let Err(error) = crate::identity::write_private(token_file, token.as_bytes())
    {
        tracing::warn!(%error, "could not remember the screen-sharing approval");
    }
    let (tx, rx) = mpsc::channel(4);
    let mut events = started.events;
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let mapped = match event {
                CaptureEvent::Frame(f) => Event::Frame(f.framebuffer, f.damage, 0),
                CaptureEvent::Cursor(c) => Event::Cursor(
                    c.and_then(|c| Cursor::from_rgba(c.width, c.height, c.hotspot, c.rgba)),
                ),
                CaptureEvent::Closed(reason) => Event::Closed(reason),
            };
            if tx.send(mapped).await.is_err() {
                return;
            }
        }
    });
    Ok(Capture {
        events: rx,
        input: Input::Wayland(started.input),
        compositor: None,
        can_control: started.can_control,
    })
}

fn start_headless(setup: HeadlessSetup) -> Result<Capture> {
    use farsight_headless::{HeadlessEvent, Options, start};
    let started = start(Options {
        width: setup.width,
        height: setup.height,
        x_display: setup.display,
        xwayland: true,
        startup: setup.startup,
        session_umask: setup.session_umask,
    })
    .context("could not start the headless desktop")?;
    let (tx, rx) = mpsc::channel(4);
    let mut events = started.events;
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let mapped = match event {
                HeadlessEvent::Ready {
                    wayland_display,
                    x_display,
                } => {
                    tracing::info!(
                        %wayland_display,
                        x_display = x_display.as_deref().unwrap_or("none"),
                        "headless desktop ready"
                    );
                    continue;
                }
                HeadlessEvent::Frame(f) => Event::Frame(f.framebuffer, f.damage, f.generation),
                HeadlessEvent::Cursor(c) => Event::Cursor(
                    c.and_then(|c| Cursor::from_rgba(c.width, c.height, c.hotspot, c.rgba)),
                ),
                HeadlessEvent::Clipboard(text) => Event::Clipboard(text),
                HeadlessEvent::Leds(leds) => Event::Leds(leds),
                HeadlessEvent::Closed(reason) => Event::Closed(reason),
            };
            if tx.send(mapped).await.is_err() {
                return;
            }
        }
    });
    Ok(Capture {
        events: rx,
        input: Input::Headless(started.handle),
        compositor: Some(started.thread),
        can_control: true,
    })
}

fn start_x11(display: Option<String>) -> Result<Capture> {
    use farsight_capture_x11::{CaptureEvent, Options, start};
    let started = start(Options {
        display,
        ..Options::default()
    })
    .context("could not share the X11 display")?;
    let (tx, rx) = mpsc::channel(4);
    let mut events = started.events;
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let mapped = match event {
                CaptureEvent::Frame(f) => Event::Frame(f.framebuffer, f.damage, 0),
                CaptureEvent::Cursor(c) => Event::Cursor(
                    c.and_then(|c| Cursor::from_rgba(c.width, c.height, c.hotspot, c.rgba)),
                ),
                CaptureEvent::Clipboard(text) => Event::Clipboard(text),
                CaptureEvent::Closed(reason) => Event::Closed(reason),
            };
            if tx.send(mapped).await.is_err() {
                return;
            }
        }
    });
    Ok(Capture {
        events: rx,
        input: Input::X11(started.input),
        compositor: None,
        can_control: true,
    })
}
