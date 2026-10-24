//! Network adapters: tokio TCP transport for viewer sessions, rustls TLS
//! (ADR-0007, M3), and later the WebSocket transport for the web gateway.
//!
//! Layer: **Adapters** (ADR-0002). Drives the sans-I/O [`Session`] over a
//! real socket. The UI talks to a running connection through
//! [`ConnectionHandle`] commands and receives [`Update`]s.
//!
//! [`Session`]: farsight_client::Session

mod viewer;

pub use viewer::{Command, ConnectError, ConnectionHandle, CursorImage, Frame, Update, connect};
