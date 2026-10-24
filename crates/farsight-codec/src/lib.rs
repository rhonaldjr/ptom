//! Pixel encodings and decoders: Raw, CopyRect, RRE, Hextile, ZRLE, Tight,
//! JPEG, and optionally H.264.
//!
//! Layer: **Entities** (ADR-0002). Pure functions over byte buffers. No I/O.
//! Every decoder bounds its work by the framebuffer size and has a fuzz
//! target, because hostile servers attack the viewer (OWASP API10).
//!
//! [`Decoder`] implements [`farsight_rfb::client::RectDecoder`] and owns the
//! viewer's [`Framebuffer`]. Decoders never apply partial results: they
//! return `Ok(None)` until the whole rectangle payload is available.

mod decoder;
pub mod encode;
mod framebuffer;
mod pixel;

pub use decoder::Decoder;
pub use framebuffer::Framebuffer;
pub use pixel::PixelConverter;
