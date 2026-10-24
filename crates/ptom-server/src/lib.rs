//! Agent-side use cases: accepting clients, enforcing access control and
//! view-only, scheduling framebuffer updates, and injecting input. Defines the
//! ports that capture backends implement (`FrameSource`, `InputSink`,
//! `ClipboardPort`, `PresenceIndicator`).
//!
//! Layer: **Use cases** (ADR-0002). No async runtime, compositor or X11 types.
//!
//! Roadmap: M4 onward.
