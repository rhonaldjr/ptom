use thiserror::Error;

/// Error returned when bytes received from a peer cannot be decoded.
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
}
