//! RFB (Remote Framebuffer) protocol entities: wire types, codecs and sans-I/O
//! state machines, implemented from RFC 6143 and the community rfbproto spec.
//!
//! Layer: **Entities** (ADR-0002). This crate must not depend on async runtimes,
//! sockets, the filesystem, or any outer-layer workspace crate.
//!
//! The connection state machines ([`client::ClientConnection`],
//! [`server::ServerConnection`]) take bytes in and return bytes and events
//! out (ADR-0015). Security types plug in through [`auth::Authenticator`];
//! rectangle payloads through [`client::RectDecoder`].

pub mod auth;
mod buffer;
pub mod client;
mod connection;
pub mod encoding;
mod error;
pub mod handshake;
pub mod limits;
pub mod messages;
pub mod pixel_format;
pub mod rect;
pub mod screen;
pub mod security;
pub mod server;
pub mod version;
pub mod wire;

pub use connection::ProtocolError;
pub use encoding::Encoding;
pub use error::DecodeError;
pub use limits::Limits;
pub use messages::{ClientMessage, CutText, RectHeader, ServerMessage};
pub use pixel_format::PixelFormat;
pub use rect::Rect;
pub use screen::Screen;
pub use security::{SecurityType, VeNCryptSubtype};
pub use version::ProtocolVersion;
