//! Viewer-side use cases: connecting, authenticating, maintaining the
//! framebuffer, and forwarding input and clipboard. Defines the ports that
//! outer layers implement (transport, credential store, discovery, display).
//!
//! Layer: **Use cases** (ADR-0002). No async runtime, UI toolkit or OS types.
//!
//! Roadmap: M1–M3.
