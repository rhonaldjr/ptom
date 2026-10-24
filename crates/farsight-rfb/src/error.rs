use thiserror::Error;

/// Error returned when bytes received from a peer cannot be decoded.
///
/// [`DecodeError::Truncated`] is not fatal for streaming decoders: it means
/// "wait for more input". Every other variant is a protocol violation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// Fewer bytes were supplied than the message requires.
    #[error("message truncated: need {needed} bytes, got {got}")]
    Truncated {
        /// Bytes required to decode the message.
        needed: usize,
        /// Bytes actually available.
        got: usize,
    },
    /// The `ProtocolVersion` message does not match `"RFB xxx.yyy\n"`.
    #[error("malformed protocol version message")]
    MalformedVersion,
    /// The peer speaks a protocol major version other than 3.
    #[error("unsupported RFB version {major}.{minor}")]
    UnsupportedVersion {
        /// Major version sent by the peer.
        major: u16,
        /// Minor version sent by the peer.
        minor: u16,
    },
    /// The pixel format violates RFC 6143 §7.4.
    #[error("invalid pixel format: {0}")]
    InvalidPixelFormat(&'static str),
    /// A message type byte that this side does not understand (or has not
    /// negotiated). The stream cannot be resynchronised after this.
    #[error("unknown message type {0}")]
    UnknownMessageType(u8),
    /// A length or count sent by the peer exceeds a configured limit.
    #[error("{what} length {len} exceeds limit {max}")]
    LimitExceeded {
        /// What was being measured.
        what: &'static str,
        /// Length announced by the peer.
        len: u64,
        /// Configured maximum.
        max: u64,
    },
    /// A rectangle lies (partly) outside the framebuffer.
    #[error("rectangle outside the framebuffer")]
    RectOutOfBounds,
    /// A rectangle uses an encoding that was not negotiated or is unsupported.
    #[error("unsupported encoding {0}")]
    UnsupportedEncoding(i32),
    /// The payload is internally inconsistent.
    #[error("malformed message: {0}")]
    Malformed(&'static str),
}

impl DecodeError {
    /// Whether this error only means "more input is needed".
    #[must_use]
    pub fn is_incomplete(&self) -> bool {
        matches!(self, Self::Truncated { .. })
    }
}
