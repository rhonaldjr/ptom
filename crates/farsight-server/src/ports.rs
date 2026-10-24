//! Ports: what the agent core needs from the outside world. Platform
//! adapters (M4 X11, M5 Wayland portal/libei, M7 headless) implement them.
//!
//! Frames flow the other way: adapters push them into [`crate::Desktop`]
//! rather than the core pulling from a frame source, so one capture feeds
//! every connected viewer.

use farsight_rfb::Screen;
use secrecy::SecretString;

/// Injects a viewer's keyboard and pointer input into the desktop.
///
/// Only called for viewers that are not view-only; the core enforces
/// view-only itself, so adapters never see input from such viewers.
pub trait InputSink: Send {
    /// A key press (`down`) or release, identified by X11 keysym.
    fn key(&mut self, down: bool, keysym: u32);

    /// Pointer position (clamped to the framebuffer the viewer knows) and
    /// button mask (bit 0 = left, 1 = middle, 2 = right, 3/4 = wheel).
    fn pointer(&mut self, buttons: u8, x: u16, y: u16);
}

/// Receives clipboard text a viewer copied.
pub trait ClipboardSink: Send {
    /// Replace the desktop clipboard with `text`. Implementations must not
    /// log it (CLAUDE.md: never log clipboard contents).
    fn set_text(&mut self, text: &str);
}

/// Checks a username and password, e.g. `farsight_vault::agent::AgentCredentials::verify`
/// (Argon2id, ADR-0014).
pub trait UserVerifier: Send + Sync {
    /// Whether the credentials are valid. Implementations must take the same
    /// time whether or not the user exists.
    fn verify(&self, username: &str, password: &SecretString) -> bool;
}

/// A cryptographically secure random source (the adapter wraps the OS
/// generator; tests use a fixed sequence).
pub trait Entropy: Send + Sync {
    /// Fills `buf` with random bytes.
    fn fill(&self, buf: &mut [u8]);
}

/// What a [`DesktopResizer`] did with a viewer's `SetDesktopSize` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeOutcome {
    /// The desktop was resized. The adapter must already have applied the
    /// new size to [`crate::Desktop`] (the reply carries the desktop's size).
    Accepted,
    /// Resizing is not allowed (the default for live desktops).
    Prohibited,
    /// The desktop could not be resized right now.
    OutOfResources,
    /// The requested screen layout is invalid.
    InvalidLayout,
}

impl ResizeOutcome {
    /// The `ExtendedDesktopSize` status code (rfbproto).
    #[must_use]
    pub fn status_code(self) -> u16 {
        match self {
            Self::Accepted => 0,
            Self::Prohibited => 1,
            Self::OutOfResources => 2,
            Self::InvalidLayout => 3,
        }
    }
}

/// Handles viewer resize requests. Live desktops have none (requests are
/// refused as prohibited); headless desktops (M7) plug one in.
pub trait DesktopResizer: Send {
    /// Tries to resize the desktop to `width` × `height` with `screens`.
    fn resize(&mut self, width: u16, height: u16, screens: &[Screen]) -> ResizeOutcome;
}
