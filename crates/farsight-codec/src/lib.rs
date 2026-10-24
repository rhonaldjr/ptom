//! Pixel encodings, decoders and (in [`encode`]) the agent's encoders: Raw, CopyRect, Hextile, ZRLE, Tight (with
//! JPEG), the Cursor, `CursorWithAlpha` and `ExtendedDesktopSize`
//! pseudo-encodings, and later H.264.
//!
//! Layer: **Entities** (ADR-0002). Pure functions over byte buffers. No I/O.
//! Every decoder bounds its work by the framebuffer size and has a fuzz
//! target, because hostile servers attack the viewer (OWASP API10).
//!
//! [`Decoder`] implements [`farsight_rfb::client::RectDecoder`] and owns the
//! viewer's [`Framebuffer`], cursor shape, screen layout and the zlib
//! streams that ZRLE and Tight keep across rectangles. Decoders never apply
//! partial results: they return `Ok(None)`, with no side effects, until the
//! whole rectangle payload is available. Inflated data is bounded by what
//! the rectangle can need (zlib-bomb protection).

mod compact;
mod cursor;
mod decoder;
mod deflate;
mod desktop;
pub mod encode;
mod framebuffer;
mod hextile;
mod inflate;
mod pixel;
mod tight;
mod util;
mod zrle;

pub use cursor::{Cursor, MAX_CURSOR_SIDE};
pub use decoder::Decoder;
pub use desktop::{DesktopResize, ResizeReason, ResizeStatus};
pub use encode::{TightEncoder, ZrleEncoder};
pub use framebuffer::Framebuffer;
pub use pixel::PixelConverter;
