//! RFB (Remote Framebuffer) protocol entities: wire types, codecs and sans-I/O
//! state machines, implemented from RFC 6143 and the community rfbproto spec.
//!
//! Layer: **Entities** (ADR-0002). This crate must not depend on async runtimes,
//! sockets, the filesystem, or any outer-layer workspace crate.

mod error;
pub mod pixel_format;
pub mod security;
pub mod version;
mod wire;

pub use error::DecodeError;
pub use pixel_format::PixelFormat;
pub use security::{SecurityType, VeNCryptSubtype};
pub use version::ProtocolVersion;
