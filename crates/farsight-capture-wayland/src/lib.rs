//! Wayland screen capture and input injection for the agent (roadmap M4).
//!
//! Capture and input both go through xdg-desktop-portal (ADR-0009): one RemoteDesktop session
//! with a ScreenCast source attached. Frames arrive over PipeWire on a dedicated OS thread and
//! are handed to the caller as [`CaptureEvent`]s; input is sent back through the same portal
//! session. Works with xdg-desktop-portal-cosmic, -gnome and -kde.
//!
//! The portal's restore token is returned in [`Started::restore_token`] so the caller can keep
//! it (decision U-3) and skip the permission dialog next time.
//!
//! Layer: **Adapters** (ADR-0002). Linux only; elsewhere [`start`] returns
//! [`CaptureError::Unsupported`].

pub mod damage;
pub mod pixels;
pub mod pointer;
pub mod throttle;

#[cfg(target_os = "linux")]
mod linux;

use tokio::sync::mpsc;

/// How many input events may wait for the portal before new ones are dropped.
const INPUT_QUEUE: usize = 1024;

/// One captured screen image.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The whole screen as `0x00RRGGBB` pixels.
    pub framebuffer: farsight_codec::Framebuffer,
    /// What changed since the previous frame; `None` means "assume everything".
    pub damage: Option<Vec<farsight_rfb::Rect>>,
}

/// The pointer image, as RGBA bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// The click point, relative to the top-left of the image.
    pub hotspot: (u16, u16),
    /// `width * height * 4` bytes, row-major, R G B A.
    pub rgba: Vec<u8>,
}

/// Something that happened to the capture.
#[derive(Debug, Clone)]
pub enum CaptureEvent {
    /// A new screen image.
    Frame(Frame),
    /// The pointer changed shape (`Some`) or was hidden (`None`).
    Cursor(Option<CursorShape>),
    /// Capture failed (e.g. the stream went away); the text says why. No
    /// events follow.
    Closed(String),
    /// The person at the desktop stopped sharing (from the desktop's own
    /// controls). A deliberate stop, not a failure. No events follow.
    Stopped(String),
}

/// How to start a capture.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// A token from an earlier [`Started::restore_token`]; lets the portal skip its dialog.
    pub restore_token: Option<String>,
    /// Ask the portal for a token that stays valid until the person revokes it.
    pub persist: bool,
    /// Only share the screen: never ask for (or hold) input control.
    pub view_only: bool,
}

/// A running capture.
#[derive(Debug)]
pub struct Started {
    /// Frames, cursor changes and the final close. Dropping it stops the capture.
    pub events: mpsc::Receiver<CaptureEvent>,
    /// Sends keyboard and pointer input to the captured desktop.
    pub input: InputHandle,
    /// The portal's new restore token, if it issued one. Store it for the next [`start`].
    pub restore_token: Option<String>,
    /// Whether input reaches the desktop. `false` when its portal only
    /// shares the screen (no RemoteDesktop): the caller must run view-only.
    pub can_control: bool,
}

/// Why capture could not start.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// This platform has no Wayland capture.
    #[error("Wayland screen capture is only available on Linux")]
    Unsupported,
    /// The desktop's portal does not implement a portal we need.
    #[error(
        "the {desktop} desktop's xdg-desktop-portal does not offer the {portal} portal, which \
         Wayland remote control needs; install xdg-desktop-portal-cosmic, -gnome or -kde"
    )]
    PortalMissing {
        /// The missing portal ("RemoteDesktop" or "ScreenCast").
        portal: &'static str,
        /// `XDG_CURRENT_DESKTOP`, or "unknown".
        desktop: String,
    },
    /// The portal will not control a keyboard and pointer.
    #[error("the {desktop} desktop's RemoteDesktop portal cannot control a keyboard and pointer")]
    DevicesUnavailable {
        /// `XDG_CURRENT_DESKTOP`, or "unknown".
        desktop: String,
    },
    /// The person at the screen declined, or closed the dialog.
    #[error("screen sharing was declined at the remote desktop")]
    Declined,
    /// The portal started without giving us a screen.
    #[error("the portal did not provide a screen to capture")]
    NoStream,
    /// A portal call failed.
    #[error("xdg-desktop-portal error: {0}")]
    Portal(String),
    /// PipeWire could not be set up.
    #[error("PipeWire error: {0}")]
    PipeWire(String),
}

/// An input event for the captured desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputEvent {
    Key { down: bool, keysym: u32 },
    KeyCode { down: bool, evdev: u16 },
    Pointer { buttons: u8, x: u16, y: u16 },
    ReleaseAll,
}

/// Sends keyboard and pointer input to the captured desktop. Cheap to clone; never blocks.
#[derive(Debug, Clone)]
pub struct InputHandle {
    tx: mpsc::Sender<InputEvent>,
}

impl InputHandle {
    pub(crate) fn channel() -> (Self, mpsc::Receiver<InputEvent>) {
        let (tx, rx) = mpsc::channel(INPUT_QUEUE);
        (Self { tx }, rx)
    }

    /// Presses (`down`) or releases an X11 keysym.
    pub fn key(&self, down: bool, keysym: u32) {
        self.send(InputEvent::Key { down, keysym });
    }

    /// Releases every key and button a viewer left pressed.
    pub fn release_all(&self) {
        self.send(InputEvent::ReleaseAll);
    }

    /// Presses or releases a physical key (Linux evdev code).
    pub fn key_code(&self, down: bool, evdev: u16) {
        self.send(InputEvent::KeyCode { down, evdev });
    }

    /// Moves the pointer to (`x`, `y`) in framebuffer pixels with RFB button mask `buttons`
    /// (bit 0 left, 1 middle, 2 right, 3/4 wheel up/down, 5/6 wheel left/right, 7 back).
    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        self.send(InputEvent::Pointer { buttons, x, y });
    }

    fn send(&self, event: InputEvent) {
        match self.tx.try_send(event) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            // Never log the event itself: it may be a keystroke.
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("input queue full; dropping an input event");
            }
        }
    }
}

/// Starts capturing the screen through xdg-desktop-portal.
///
/// Unless `options.restore_token` is still valid, this shows the portal's permission dialog
/// and waits for the person at the screen to answer.
///
/// # Errors
/// [`CaptureError`] if the portal is missing, the request is declined, or PipeWire fails.
pub async fn start(options: Options) -> Result<Started, CaptureError> {
    #[cfg(target_os = "linux")]
    {
        linux::start(options).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = options;
        Err(CaptureError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn input_handle_queues_without_blocking() {
        let (handle, mut rx) = InputHandle::channel();
        handle.key(true, 0x61);
        handle.clone().pointer(1, 10, 20);
        assert_eq!(
            rx.recv().await,
            Some(InputEvent::Key {
                down: true,
                keysym: 0x61
            })
        );
        assert_eq!(
            rx.recv().await,
            Some(InputEvent::Pointer {
                buttons: 1,
                x: 10,
                y: 20
            })
        );
    }

    #[test]
    fn input_handle_drops_when_full_or_closed() {
        let (handle, rx) = InputHandle::channel();
        for _ in 0..INPUT_QUEUE + 10 {
            handle.key(true, 1);
        }
        drop(rx);
        handle.key(false, 1);
    }

    #[test]
    fn missing_portal_error_names_the_desktop() {
        let error = CaptureError::PortalMissing {
            portal: "RemoteDesktop",
            desktop: "COSMIC".into(),
        };
        let text = error.to_string();
        assert!(
            text.contains("COSMIC") && text.contains("RemoteDesktop"),
            "{text}"
        );
    }

    /// Q3 manual charter: docs/testing/charters/2026-10-24-wayland-capture.md.
    /// Run in a Wayland session with `cargo test -p farsight-capture-wayland -- --ignored`.
    #[tokio::test]
    #[ignore = "needs a live Wayland session and a person to answer the portal dialog"]
    async fn live_portal_smoke() {
        let started = start(Options {
            restore_token: None,
            persist: true,
            view_only: false,
        })
        .await
        .unwrap();
        println!("restore token issued: {}", started.restore_token.is_some());
        let mut events = started.events;
        let mut frames = 0;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        started.input.pointer(0, 10, 10);
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
            match event {
                CaptureEvent::Frame(frame) => {
                    frames += 1;
                    println!(
                        "frame {}x{} damage={:?}",
                        frame.framebuffer.width(),
                        frame.framebuffer.height(),
                        frame.damage.as_ref().map(Vec::len)
                    );
                }
                CaptureEvent::Cursor(shape) => {
                    println!("cursor {:?}", shape.map(|s| (s.width, s.height, s.hotspot)));
                }
                CaptureEvent::Closed(reason) | CaptureEvent::Stopped(reason) => {
                    panic!("closed: {reason}")
                }
            }
        }
        assert!(frames > 0, "no frames in 5 s");
    }
}
