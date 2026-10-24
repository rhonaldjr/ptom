//! Viewer-side use cases: connecting, authenticating, maintaining the
//! framebuffer, and forwarding input and clipboard.
//!
//! Layer: **Use cases** (ADR-0002). No async runtime, UI toolkit or OS types:
//! adapters move bytes in and out of a [`Session`] and render its framebuffer.

pub mod address;
pub mod keysym;
pub mod options;
mod session;

pub use address::ServerAddress;
pub use options::ViewerOptions;
pub use session::{Fingerprint, IdentityKind, Session, SessionConfig, SessionEvent, SessionStatus};
