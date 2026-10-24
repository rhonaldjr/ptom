//! Agent-side use cases: accepting clients, enforcing access control and
//! view-only, scheduling framebuffer updates, and injecting input.
//!
//! Layer: **Use cases** (ADR-0002). No async runtime, sockets, clocks,
//! randomness sources, compositor or X11 types: everything from the outside
//! world comes in through the [`ports`] traits or as plain values, so the
//! whole agent core runs (and is tested) in memory, sans-I/O (ADR-0015).
//!
//! - [`Desktop`]: the shared model a capture adapter feeds frames, damage,
//!   cursor shapes and clipboard text into.
//! - [`ServerSession`]: one per viewer; wraps the RFB state machine, picks
//!   the security offer, runs the owner/accept prompt (U-2), applies the
//!   viewer's pixel format and encodings and sends updates from the
//!   [`Desktop`].
//! - [`AccessControl`]: per-agent connection limits, failed-login lockout
//!   and the shared/exclusive policy.
//!
//! Roadmap: M4 onward.

pub mod access;
pub mod damage;
mod desktop;
pub mod ports;
mod security;
mod session;
mod update;

pub use access::{AccessControl, AccessPolicy, Admission, ConnectionId, Refusal, SharePolicy};
pub use damage::Region;
pub use desktop::Desktop;
pub use ports::{ClipboardSink, DesktopResizer, Entropy, InputSink, ResizeOutcome, UserVerifier};
pub use session::{ServerSession, ServerSessionConfig, SessionEvent, SessionPorts};
