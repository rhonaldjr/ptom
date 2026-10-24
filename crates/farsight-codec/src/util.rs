//! Small helpers shared by the rectangle decoders.

use farsight_rfb::DecodeError;
use farsight_rfb::wire::Reader;

/// Unwraps a read from the wire, turning "more input needed" into an early
/// `Ok(None)` from the enclosing decoder.
macro_rules! need {
    ($read:expr) => {
        match $read {
            Ok(value) => value,
            Err(e) if e.is_incomplete() => return Ok(None),
            Err(e) => return Err(e),
        }
    };
}
pub(crate) use need;

/// For data that is already complete (inflated zlib output): running out of
/// bytes is a malformed payload, not a reason to wait.
pub(crate) fn complete<T>(read: Result<T, DecodeError>) -> Result<T, DecodeError> {
    read.map_err(|e| {
        if e.is_incomplete() {
            DecodeError::Malformed("compressed data ended early")
        } else {
            e
        }
    })
}

/// Reads the Tight "compact length": 1 to 3 bytes, 7 + 7 + 8 bits, least
/// significant group first.
pub(crate) fn compact_length(r: &mut Reader<'_>) -> Result<usize, DecodeError> {
    let first = r.u8()?;
    let mut len = usize::from(first & 0x7f);
    if first & 0x80 != 0 {
        let second = r.u8()?;
        len |= usize::from(second & 0x7f) << 7;
        if second & 0x80 != 0 {
            len |= usize::from(r.u8()?) << 14;
        }
    }
    Ok(len)
}

/// `a * b`, as a [`DecodeError`] if it overflows.
pub(crate) fn mul(a: usize, b: usize) -> Result<usize, DecodeError> {
    a.checked_mul(b)
        .ok_or(DecodeError::Malformed("size overflow"))
}

/// `a + b`, as a [`DecodeError`] if it overflows.
pub(crate) fn add(a: usize, b: usize) -> Result<usize, DecodeError> {
    a.checked_add(b)
        .ok_or(DecodeError::Malformed("size overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_length_matches_spec_examples() {
        for (bytes, value) in [
            (&[0x00][..], 0),
            (&[0x7f][..], 127),
            (&[0x80, 0x01][..], 128),
            (&[0x90, 0x4e][..], 10000),
            (&[0xff, 0x7f][..], 16383),
            (&[0x80, 0x80, 0x01][..], 16384),
            (&[0xff, 0xff, 0xff][..], 4_194_303),
        ] {
            let mut r = Reader::new(bytes);
            assert_eq!(compact_length(&mut r), Ok(value));
            assert_eq!(r.remaining(), 0);
        }
        assert!(
            compact_length(&mut Reader::new(&[0x80]))
                .unwrap_err()
                .is_incomplete()
        );
        assert!(
            compact_length(&mut Reader::new(&[0x80, 0x80]))
                .unwrap_err()
                .is_incomplete()
        );
    }
}
