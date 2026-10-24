//! Resource limits applied to peer-controlled lengths (OWASP API4).

/// Upper bounds on lengths and counts a peer may announce.
///
/// Every decoder checks the announced value against these limits before
/// allocating or waiting for the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Maximum clipboard text accepted in one message. Larger texts are
    /// skipped (streamed past, not buffered) rather than failing the session.
    pub max_cut_text: u32,
    /// Maximum entries in a `SetEncodings` message.
    pub max_encodings: u16,
    /// Maximum desktop name length.
    pub max_name: u32,
    /// Maximum length of a failure-reason string.
    pub max_reason: u32,
    /// Maximum framebuffer width or height.
    pub max_framebuffer_side: u16,
}

impl Limits {
    /// Defaults suitable for both viewer and agent.
    pub const DEFAULT: Self = Self {
        max_cut_text: 16 * 1024 * 1024,
        max_encodings: 1024,
        max_name: 64 * 1024,
        max_reason: 64 * 1024,
        max_framebuffer_side: 16384,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}
