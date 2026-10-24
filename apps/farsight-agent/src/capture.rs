//! One interface over the capture backends (ADR-0018).

use std::path::Path;

#[cfg(feature = "wayland")]
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use farsight_codec::{Cursor, Framebuffer};
use farsight_rfb::Rect;
use tokio::sync::mpsc;

use crate::options::Backend;

/// Something the desktop backend produced.
#[cfg_attr(not(feature = "wayland"), allow(dead_code))]
pub enum Event {
    Frame(Framebuffer, Option<Vec<Rect>>),
    Cursor(Option<Cursor>),
    #[allow(dead_code)] // Produced by the X11 backend (M5).
    Clipboard(String),
    Closed(String),
}

/// Sends input to the desktop.
#[derive(Clone)]
pub enum Input {
    #[cfg(feature = "wayland")]
    Wayland(farsight_capture_wayland::InputHandle),
    /// A build without any backend (never started; see [`start`]).
    #[cfg(not(feature = "wayland"))]
    #[allow(dead_code)]
    Unavailable,
}

impl Input {
    pub fn key(&self, down: bool, keysym: u32) {
        match self {
            #[cfg(feature = "wayland")]
            Self::Wayland(i) => i.key(down, keysym),
            #[cfg(not(feature = "wayland"))]
            Self::Unavailable => {
                let _ = (down, keysym);
            }
        }
    }

    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        match self {
            #[cfg(feature = "wayland")]
            Self::Wayland(i) => i.pointer(buttons, x, y),
            #[cfg(not(feature = "wayland"))]
            Self::Unavailable => {
                let _ = (buttons, x, y);
            }
        }
    }

    pub fn set_clipboard(&self, text: &str) {
        // The portal has no clipboard yet (planned with the Clipboard portal).
        let _ = (self, text);
    }
}

/// A running backend.
pub struct Capture {
    pub events: mpsc::Receiver<Event>,
    pub input: Input,
}

/// Starts the backend for `backend` (already resolved from `Auto`).
/// `token_file` keeps the Wayland portal approval between runs (U-3).
#[cfg_attr(not(feature = "wayland"), allow(clippy::unused_async))]
pub async fn start(backend: Backend, token_file: &Path) -> Result<Capture> {
    match backend {
        Backend::X11 | Backend::Auto => bail!("sharing an X11 desktop arrives in milestone M5"),
        #[cfg(feature = "wayland")]
        Backend::Wayland => start_wayland(token_file).await,
        #[cfg(not(feature = "wayland"))]
        Backend::Wayland => {
            let _ = token_file;
            bail!("this build has no Wayland support; rebuild with `--features wayland`")
        }
    }
}

#[cfg(feature = "wayland")]
async fn start_wayland(token_file: &Path) -> Result<Capture> {
    use farsight_capture_wayland::{CaptureEvent, Options, start};
    let restore_token = std::fs::read_to_string(token_file)
        .ok()
        .map(|t| t.trim().to_owned());
    let started = start(Options {
        restore_token,
        persist: true,
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
                CaptureEvent::Frame(f) => Event::Frame(f.framebuffer, f.damage),
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
    })
}
