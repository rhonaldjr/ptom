//! A headless Wayland compositor for persistent remote desktops (roadmap M7,
//! ADR-0006).
//!
//! [`start`] runs a Smithay compositor on its own thread. It has no screen:
//! it renders with pixman (software, no GPU or DRM access) into an off-screen
//! image the size of the remote desktop, and runs Xwayland so X11 apps work
//! too. Once both are ready it runs the user's startup script
//! ([`Options::startup`], user decision U-10) with `WAYLAND_DISPLAY` and
//! `DISPLAY` pointing at the new desktop.
//!
//! Frames, cursor shapes and clipboard text arrive on [`Started::events`];
//! input, clipboard text and resize requests go through [`Handle`].
//!
//! Window management is deliberately simple: windows float, new ones are
//! cascaded, a click focuses and raises, and clients can ask to be moved.

mod clipboard;
mod focus;
mod grab;
mod handlers;
mod keys;
mod layout;
mod render;
mod session;
mod state;

use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use farsight_codec::Framebuffer;
use farsight_rfb::{Rect, Screen};
use smithay::reexports::calloop::channel;

pub use layout::{LayoutError, validate_layout};
pub use session::{DEFAULT_STARTUP_SCRIPT, xwayland_supported};

/// Largest clipboard text exchanged in either direction (1 MiB).
pub const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

/// Largest desktop side the compositor accepts.
pub const MAX_SIDE: u16 = 8192;

/// How long [`Handle::resize`] waits for the compositor to answer.
const RESIZE_TIMEOUT: Duration = Duration::from_secs(2);

/// Capacity of the event channel. Frames are dropped (and their damage kept
/// for the next frame) rather than queued beyond this.
const EVENT_CHANNEL_CAPACITY: usize = 4;

/// A rendered screen update.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The whole desktop as it is now.
    pub framebuffer: Framebuffer,
    /// The areas that changed since the previous frame, or `None` when the
    /// whole framebuffer should be treated as changed (first frame, resize).
    pub damage: Option<Vec<Rect>>,
    /// Increases with every resize, so frames rendered before a resize can be
    /// told apart from those after it.
    pub generation: u64,
}

/// A cursor image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorShape {
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// The pointer's hot spot within the image.
    pub hotspot: (u16, u16),
    /// Straight (non-premultiplied) RGBA, row-major, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Something the compositor produced.
#[derive(Debug, Clone)]
pub enum HeadlessEvent {
    /// The desktop is up: clients can connect to these displays.
    Ready {
        /// `WAYLAND_DISPLAY` for the desktop.
        wayland_display: String,
        /// `DISPLAY` for X11 apps, if Xwayland started.
        x_display: Option<String>,
    },
    /// A screen update.
    Frame(Frame),
    /// A new cursor shape, or `None` when the cursor is hidden.
    Cursor(Option<CursorShape>),
    /// Text an app put on the clipboard.
    Clipboard(String),
    /// The compositor stopped; no more events follow.
    Closed(String),
}

/// Compositor options.
#[derive(Debug, Clone)]
pub struct Options {
    /// Initial desktop width.
    pub width: u16,
    /// Initial desktop height.
    pub height: u16,
    /// X display number to use for Xwayland (`:N`); `None` picks a free one.
    pub x_display: Option<u32>,
    /// Whether to run Xwayland at all.
    pub xwayland: bool,
    /// Script to run once the desktop is up (U-10). `None` runs nothing.
    pub startup: Option<PathBuf>,
    /// The umask the session's apps get (the caller may run with a stricter one).
    pub session_umask: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            x_display: None,
            xwayland: true,
            startup: None,
            session_umask: None,
        }
    }
}

/// What happened to a resize request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeResult {
    /// The desktop now has the requested layout; frames from now on carry
    /// this `generation` or a later one.
    Resized {
        /// The layout generation after the resize.
        generation: u64,
    },
    /// The layout is invalid (overlapping, off the desktop, too large…).
    InvalidLayout,
    /// The compositor could not resize right now.
    Failed,
}

/// Commands sent to the compositor thread.
pub(crate) enum Command {
    Key {
        down: bool,
        keysym: u32,
    },
    Pointer {
        buttons: u8,
        x: u16,
        y: u16,
    },
    Clipboard(String),
    Viewers(usize),
    ReleaseAll,
    Resize {
        width: u16,
        height: u16,
        screens: Vec<Screen>,
        reply: std_mpsc::Sender<ResizeResult>,
    },
    Stop,
}

impl std::fmt::Debug for Command {
    /// Never shows keys or clipboard text (CLAUDE.md: don't log them).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Key { .. } => "Key",
            Self::Pointer { .. } => "Pointer",
            Self::Clipboard(_) => "Clipboard",
            Self::Viewers(_) => "Viewers",
            Self::ReleaseAll => "ReleaseAll",
            Self::Resize { .. } => "Resize",
            Self::Stop => "Stop",
        })
    }
}

/// Sends input, clipboard text and resize requests to the compositor.
/// Cloning is cheap; every clone talks to the same compositor.
#[derive(Clone, Debug)]
pub struct Handle {
    commands: channel::Sender<Command>,
}

impl Handle {
    /// A key press (`down`) or release, identified by X11 keysym.
    pub fn key(&self, down: bool, keysym: u32) {
        let _ = self.commands.send(Command::Key { down, keysym });
    }

    /// Pointer position and RFB button mask (bit 0 = left, 1 = middle,
    /// 2 = right, 3/4 = wheel up/down, 5/6 = wheel left/right).
    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        let _ = self.commands.send(Command::Pointer { buttons, x, y });
    }

    /// Replaces the desktop clipboard. Text over [`MAX_CLIPBOARD_BYTES`] is ignored.
    pub fn set_clipboard(&self, text: String) {
        if text.len() <= MAX_CLIPBOARD_BYTES {
            let _ = self.commands.send(Command::Clipboard(text));
        }
    }

    /// Releases every key and button held down, e.g. when control passes
    /// to another viewer.
    pub fn release_all(&self) {
        let _ = self.commands.send(Command::ReleaseAll);
    }

    /// How many viewers are connected: with two or more, a coloured frame
    /// around the desktop tells everyone in it that others are watching.
    pub fn set_viewers(&self, count: usize) {
        let _ = self.commands.send(Command::Viewers(count));
    }

    /// Resizes the desktop to `width` × `height` laid out as `screens`
    /// (one screen covering everything when empty). Blocks for up to two
    /// seconds while the compositor applies it.
    #[must_use]
    pub fn resize(&self, width: u16, height: u16, screens: &[Screen]) -> ResizeResult {
        let (reply, answer) = std_mpsc::channel();
        let command = Command::Resize {
            width,
            height,
            screens: screens.to_vec(),
            reply,
        };
        if self.commands.send(command).is_err() {
            return ResizeResult::Failed;
        }
        answer
            .recv_timeout(RESIZE_TIMEOUT)
            .unwrap_or(ResizeResult::Failed)
    }

    /// Stops the compositor (the startup session gets disconnected).
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }
}

/// A running compositor.
#[derive(Debug)]
pub struct Started {
    /// `Ready`, frames, cursor shapes, clipboard text and the final `Closed`.
    pub events: tokio::sync::mpsc::Receiver<HeadlessEvent>,
    /// Input, clipboard and resizing.
    pub handle: Handle,
    /// The compositor thread; join it after [`Handle::stop`] so the
    /// session's apps are cleaned up before the process exits.
    pub thread: std::thread::JoinHandle<()>,
}

/// Why the compositor could not start.
#[derive(Debug, thiserror::Error)]
pub enum HeadlessError {
    /// The requested size is zero or larger than [`MAX_SIDE`].
    #[error("invalid desktop size {0}x{1}")]
    InvalidSize(u16, u16),
    /// The compositor thread could not be started.
    #[error("could not start the compositor: {0}")]
    Start(String),
}

/// Starts the compositor on its own thread. Returns once its Wayland socket
/// exists; [`HeadlessEvent::Ready`] follows when Xwayland is up too.
///
/// # Errors
/// [`HeadlessError`] if the size is invalid or the compositor fails to start.
pub fn start(options: Options) -> Result<Started, HeadlessError> {
    if !layout::valid_size(options.width, options.height) {
        return Err(HeadlessError::InvalidSize(options.width, options.height));
    }
    let (events_tx, events) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let (commands, commands_rx) = channel::channel();
    let (started_tx, started_rx) = std_mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("farsight-headless".into())
        .spawn(move || state::run(&options, events_tx, commands_rx, &started_tx))
        .map_err(|e| HeadlessError::Start(e.to_string()))?;
    match started_rx.recv() {
        Ok(Ok(())) => Ok(Started {
            events,
            handle: Handle { commands },
            thread,
        }),
        Ok(Err(reason)) => Err(HeadlessError::Start(reason)),
        Err(_) => Err(HeadlessError::Start("the compositor thread exited".into())),
    }
}
