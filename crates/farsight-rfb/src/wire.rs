//! Bounds-checked primitives for reading and writing RFB wire data.
//!
//! All multi-byte integers on the wire are big-endian (RFC 6143 §7).

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

/// A cursor over received bytes. Every read is bounds-checked and reports
/// [`DecodeError::Truncated`] when more input is needed.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Starts reading at the beginning of `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes consumed so far.
    #[must_use]
    pub fn consumed(&self) -> usize {
        self.pos
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Borrows the next `len` bytes.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than `len` bytes remain.
    pub fn bytes(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.saturating_add(len);
        let slice = self.buf.get(self.pos..end).ok_or(DecodeError::Truncated {
            needed: end,
            got: self.buf.len(),
        })?;
        self.pos = end;
        Ok(slice)
    }

    /// Reads the next `N` bytes as an array.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than `N` bytes remain.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    /// Skips `len` bytes of padding.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than `len` bytes remain.
    pub fn skip(&mut self, len: usize) -> Result<(), DecodeError> {
        self.bytes(len).map(|_| ())
    }

    /// Reads a `u8`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if no bytes remain.
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(u8::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `u16`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than 2 bytes remain.
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `u32`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than 4 bytes remain.
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `i32`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than 4 bytes remain.
    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    /// Reads a `u32` length prefix and checks it against `max` *before* any
    /// allocation or wait, so a hostile peer can't make us buffer gigabytes.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if the prefix is incomplete, or
    /// [`DecodeError::LimitExceeded`] if the length exceeds `max`.
    pub fn length(&mut self, what: &'static str, max: u32) -> Result<usize, DecodeError> {
        let len = self.u32()?;
        if len > max {
            return Err(DecodeError::LimitExceeded {
                what,
                len: u64::from(len),
                max: u64::from(max),
            });
        }
        Ok(len as usize)
    }
}

/// Big-endian writers for `Vec<u8>` output buffers.
pub trait WriteExt {
    /// Appends a `u8`.
    fn put_u8(&mut self, value: u8);
    /// Appends a big-endian `u16`.
    fn put_u16(&mut self, value: u16);
    /// Appends a big-endian `u32`.
    fn put_u32(&mut self, value: u32);
    /// Appends a big-endian `i32`.
    fn put_i32(&mut self, value: i32);
    /// Appends `count` zero bytes of padding.
    fn put_padding(&mut self, count: usize);
}

impl WriteExt for Vec<u8> {
    fn put_u8(&mut self, value: u8) {
        self.push(value);
    }
    fn put_u16(&mut self, value: u16) {
        self.extend_from_slice(&value.to_be_bytes());
    }
    fn put_u32(&mut self, value: u32) {
        self.extend_from_slice(&value.to_be_bytes());
    }
    fn put_i32(&mut self, value: i32) {
        self.extend_from_slice(&value.to_be_bytes());
    }
    fn put_padding(&mut self, count: usize) {
        self.resize(self.len() + count, 0);
    }
}

/// Converts a length to the `u32` used on the wire.
///
/// # Errors
/// [`DecodeError::LimitExceeded`] if `len` does not fit in a `u32`.
pub fn wire_length(what: &'static str, len: usize) -> Result<u32, DecodeError> {
    u32::try_from(len).map_err(|_| DecodeError::LimitExceeded {
        what,
        len: len as u64,
        max: u64::from(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian_integers() {
        let mut r = Reader::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0xff, 0xff, 0xff, 0xfe]);
        assert_eq!(r.u8(), Ok(1));
        assert_eq!(r.u16(), Ok(0x0203));
        assert_eq!(r.u16(), Ok(0x0405));
        assert_eq!(r.i32(), Ok(-2));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn truncation_reports_total_needed() {
        let mut r = Reader::new(&[0, 1]);
        r.u8().unwrap();
        assert_eq!(r.u32(), Err(DecodeError::Truncated { needed: 5, got: 2 }));
    }

    #[test]
    fn length_prefix_is_checked_before_waiting_for_data() {
        let mut r = Reader::new(&[0, 0x10, 0, 0]);
        assert_eq!(
            r.length("cut text", 1024),
            Err(DecodeError::LimitExceeded {
                what: "cut text",
                len: 0x0010_0000,
                max: 1024
            })
        );
    }
}
