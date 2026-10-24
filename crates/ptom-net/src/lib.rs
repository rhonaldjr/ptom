//! Network adapters: tokio TCP listeners/connectors, rustls TLS (ADR-0007),
//! and later the WebSocket transport for the web gateway (Horizon).
//!
//! Layer: **Adapters** (ADR-0002). Drives the sans-I/O state machines from
//! `ptom-rfb` over real sockets.
//!
//! Roadmap: M1 (TCP), M3 (TLS).
