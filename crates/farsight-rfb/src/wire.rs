//! Bounds-checked helpers for reading fixed-size wire messages.

use crate::DecodeError;

/// Borrows the first `N` bytes of `bytes` as an array, or reports truncation.
pub(crate) fn take<const N: usize>(bytes: &[u8]) -> Result<&[u8; N], DecodeError> {
    bytes
        .get(..N)
        .and_then(|prefix| prefix.try_into().ok())
        .ok_or(DecodeError::Truncated {
            needed: N,
            got: bytes.len(),
        })
}
