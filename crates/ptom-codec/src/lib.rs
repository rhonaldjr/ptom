//! Pixel encodings and decoders: Raw, CopyRect, RRE, Hextile, ZRLE, Tight,
//! JPEG, and optionally H.264.
//!
//! Layer: **Entities** (ADR-0002). Pure functions over byte buffers. No I/O.
//! Every decoder must bound its allocations by the framebuffer size and must
//! have a fuzz target (hostile servers attack the viewer: OWASP API10).
//!
//! Roadmap: decoders in M2, encoders in M4/M8.
