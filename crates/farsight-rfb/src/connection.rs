//! Types shared by the client and server connection state machines.

use thiserror::Error;

use crate::auth::AuthError;
use crate::{DecodeError, ProtocolVersion, SecurityType};

/// A fatal connection error. After any error the connection is closed;
/// callers should still flush pending output (it may carry a failure reason).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProtocolError {
    /// The peer sent bytes that violate the protocol.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// Authentication failed.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// The server refused the connection or the authentication.
    #[error("server refused the connection: {0}")]
    Refused(String),
    /// The caller used the API out of order (e.g. sending input before the
    /// handshake finished).
    #[error("invalid operation in the current state: {0}")]
    InvalidState(&'static str),
}

/// Whether a `SecurityResult` message follows the given security type
/// (RFC 6143 §7.1.3: always in 3.8; in 3.3 and 3.7, not for `None`).
pub(crate) fn has_security_result(version: ProtocolVersion, security: SecurityType) -> bool {
    version == ProtocolVersion::V3_8 || security != SecurityType::None
}

/// `SecurityResult` status codes.
pub(crate) const SECURITY_RESULT_OK: u32 = 0;
pub(crate) const SECURITY_RESULT_FAILED: u32 = 1;
