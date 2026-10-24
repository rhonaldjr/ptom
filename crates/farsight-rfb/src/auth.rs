//! The interface between the connection state machines and security-type
//! implementations (which live in `farsight-security`).
//!
//! An authenticator runs the security-type-specific part of the handshake on
//! the raw byte stream. Some types change the transport partway through:
//! VeNCrypt switches to TLS, and RA2 switches to an AES-EAX framed stream.
//! The authenticator signals this with [`AuthProgress::UpgradeTransport`] and
//! the caller performs the upgrade before feeding further bytes.

use thiserror::Error;

/// Why authentication failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("authentication failed: {reason}")]
pub struct AuthError {
    /// Human-readable reason (sent to the client in RFB 3.8).
    pub reason: String,
}

impl AuthError {
    /// Creates an error with `reason`.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// A transport change requested by a security type.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransportUpgrade {
    /// Start a TLS session (VeNCrypt X509 subtypes). All later bytes, in both
    /// directions, flow inside TLS.
    Tls,
}

/// Result of one authenticator step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthProgress {
    /// Waiting for more input from the peer.
    NeedMore,
    /// The caller must upgrade the transport, then call `advance` again.
    UpgradeTransport(TransportUpgrade),
    /// The security type's exchange is complete.
    Complete,
}

/// One side of a security type's handshake, driven over raw bytes.
pub trait Authenticator: Send {
    /// Consumes bytes from `input` (returning how many) and appends any reply
    /// to `output`. Called once with empty input right after the security type
    /// is chosen, so a side that speaks first can do so.
    ///
    /// # Errors
    /// [`AuthError`] if the peer's data is invalid or the credentials are wrong.
    fn advance(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError>;
}

/// The `None` security type: nothing to exchange.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAuth;

impl Authenticator for NoAuth {
    fn advance(
        &mut self,
        _input: &[u8],
        _output: &mut Vec<u8>,
    ) -> Result<(usize, AuthProgress), AuthError> {
        Ok((0, AuthProgress::Complete))
    }
}
