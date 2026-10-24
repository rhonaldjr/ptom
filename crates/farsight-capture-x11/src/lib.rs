//! X11 screen capture and input injection for the Farsight agent
//! (x0vncserver parity, roadmap M5).
//!
//! [`start`] connects to an X display and spawns three threads, each with its
//! own X connection, so nothing here blocks the tokio runtime:
//!
//! - **capture**: XDamage-driven grabs (MIT-SHM when available, else
//!   `GetImage`), or 32×32 tile polling without XDamage; RandR / root resize
//!   handling; XFixes cursor shapes.
//! - **input**: XTest key and pointer injection.
//! - **clipboard**: owns `CLIPBOARD` for the remote side and watches it with
//!   XFixes selection notifications.
//!
//! Frames, cursor shapes and clipboard text arrive on [`Started::events`];
//! input and outgoing clipboard text go through [`InputHandle`].
//!
//! Pure protocol and X11 extension specs only (ADR-0011): no Xlib, no code
//! from other VNC servers.

mod buttons;
mod capture;
mod clipboard;
mod grab;
mod input;
mod keys;
mod pixels;
mod region;
mod wait;

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};

/// Largest clipboard text exchanged in either direction (1 MiB).
pub const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

/// Capacity of the event channel. Frames are dropped (and their damage kept
/// for the next frame) rather than queued beyond this.
const EVENT_CHANNEL_CAPACITY: usize = 4;

/// A captured screen update.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The whole screen as it is now.
    pub framebuffer: farsight_codec::Framebuffer,
    /// The areas that changed since the previous frame, or `None` when the
    /// whole framebuffer should be treated as changed (first frame, resize).
    pub damage: Option<Vec<farsight_rfb::Rect>>,
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

/// Something the backend observed.
#[derive(Debug, Clone)]
pub enum CaptureEvent {
    /// A screen update.
    Frame(Frame),
    /// A new cursor shape, or `None` when the cursor is hidden/empty.
    Cursor(Option<CursorShape>),
    /// Text another X client put on the `CLIPBOARD` selection.
    Clipboard(String),
    /// The X connection ended; no more events follow.
    Closed(String),
}

/// Backend options.
#[derive(Debug, Clone)]
pub struct Options {
    /// X display to open, e.g. `":0"`. `None` uses `$DISPLAY`.
    pub display: Option<String>,
    /// Polling interval used when the server lacks XDamage.
    pub poll_interval: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            display: None,
            poll_interval: Duration::from_millis(100),
        }
    }
}

/// A running backend.
#[derive(Debug)]
pub struct Started {
    /// Frames, cursor shapes, clipboard text and the final `Closed`.
    pub events: tokio::sync::mpsc::Receiver<CaptureEvent>,
    /// Injects input and sets the clipboard.
    pub input: InputHandle,
}

/// Why the backend could not start.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The X display could not be opened.
    #[error("cannot open X display: {0}")]
    Connect(#[from] ConnectError),
    /// The X connection failed.
    #[error("X connection error: {0}")]
    Connection(#[from] ConnectionError),
    /// An X request failed.
    #[error("X request failed: {0}")]
    Reply(#[from] ReplyError),
    /// An X request failed or no resource ID was available.
    #[error("X request failed: {0}")]
    ReplyOrId(#[from] ReplyOrIdError),
    /// A required X extension is missing.
    #[error("the X server lacks the {0} extension")]
    MissingExtension(&'static str),
    /// The root window's visual cannot be converted to RGB.
    #[error("unsupported X visual: {0}")]
    UnsupportedVisual(String),
    /// A worker thread could not be started.
    #[error("cannot start worker thread: {0}")]
    Io(#[from] std::io::Error),
}

impl CaptureError {
    /// Whether the X connection itself is gone (as opposed to one request failing).
    pub(crate) fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Connect(_)
                | Self::Connection(_)
                | Self::Io(_)
                | Self::Reply(ReplyError::ConnectionError(_))
                | Self::ReplyOrId(ReplyOrIdError::ConnectionError(_))
        )
    }
}

/// An input request for the input thread.
#[derive(Debug, Clone, Copy)]
enum InputCommand {
    Key { down: bool, keysym: u32 },
    Pointer { buttons: u8, x: u16, y: u16 },
}

/// Sends input and clipboard text to the X server. Cheap to clone; never blocks.
#[derive(Debug, Clone)]
pub struct InputHandle {
    input: mpsc::Sender<InputCommand>,
    clipboard: mpsc::Sender<String>,
    clipboard_wake: Arc<UnixStream>,
}

impl InputHandle {
    /// Presses (`down`) or releases a key given as an X keysym.
    pub fn key(&self, down: bool, keysym: u32) {
        self.send(InputCommand::Key { down, keysym });
    }

    /// Moves the pointer to (`x`, `y`) and sets the buttons to the RFB mask
    /// `buttons`: bit 0 left, 1 middle, 2 right, 3/4 wheel up/down,
    /// 5/6 wheel left/right, 7 back (X button 8).
    pub fn pointer(&self, buttons: u8, x: u16, y: u16) {
        self.send(InputCommand::Pointer { buttons, x, y });
    }

    /// Takes ownership of the X `CLIPBOARD` selection with `text`.
    /// Text over [`MAX_CLIPBOARD_BYTES`] is ignored.
    pub fn set_clipboard(&self, text: String) {
        if text.len() > MAX_CLIPBOARD_BYTES {
            tracing::warn!(bytes = text.len(), "clipboard text too large; ignored");
            return;
        }
        if self.clipboard.send(text).is_ok() {
            wait::wake(&self.clipboard_wake);
        }
    }

    fn send(&self, command: InputCommand) {
        if self.input.send(command).is_err() {
            tracing::debug!("input thread has stopped; input dropped");
        }
    }
}

/// Connects to the X display and starts capturing.
///
/// # Errors
/// [`CaptureError`] if the display cannot be opened, lacks XTest, has an
/// unsupported root visual, or a worker thread cannot be spawned.
pub fn start(options: Options) -> Result<Started, CaptureError> {
    let Options {
        display,
        poll_interval,
    } = options;
    let display = display.as_deref();
    let (events_tx, events) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);

    let capturer = capture::Capturer::connect(display, poll_interval)?;
    let injector = input::Injector::connect(display)?;
    let (wake_tx, wake_rx) = UnixStream::pair()?;
    wake_tx.set_nonblocking(true)?;
    wake_rx.set_nonblocking(true)?;
    let clipboard = clipboard::ClipboardOwner::connect(display, wake_rx)?;

    let (input_tx, input_rx) = mpsc::channel();
    let (clip_tx, clip_rx) = mpsc::channel();

    spawn("farsight-x11-input", move || injector.run(&input_rx))?;
    if let Some(clipboard) = clipboard {
        let tx = events_tx.clone();
        spawn("farsight-x11-clipboard", move || {
            clipboard.run(&tx, &clip_rx);
        })?;
    }
    spawn("farsight-x11-capture", move || capturer.run(&events_tx))?;

    Ok(Started {
        events,
        input: InputHandle {
            input: input_tx,
            clipboard: clip_tx,
            clipboard_wake: Arc::new(wake_tx),
        },
    })
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> Result<(), CaptureError> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)?;
    Ok(())
}
