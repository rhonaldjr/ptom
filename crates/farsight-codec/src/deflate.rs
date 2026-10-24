//! Persistent zlib compressors for the server-side encoders: the peer keeps
//! one inflate stream per compressor, so the dictionary carries across
//! rectangles and every chunk ends with a sync flush.

use std::fmt;

use farsight_rfb::DecodeError;
use flate2::{Compress, Compression, FlushCompress};

/// Highest zlib compression level.
pub(crate) const MAX_LEVEL: u8 = 9;

/// One zlib stream whose state persists across chunks.
pub(crate) struct ZlibCompressor {
    inner: Compress,
}

impl fmt::Debug for ZlibCompressor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZlibCompressor")
            .field("total_in", &self.inner.total_in())
            .field("total_out", &self.inner.total_out())
            .finish()
    }
}

impl ZlibCompressor {
    /// Starts a stream at compression `level` (0 to 9; higher is clamped).
    pub(crate) fn new(level: u8) -> Self {
        Self {
            inner: Compress::new(Compression::new(u32::from(level.min(MAX_LEVEL))), true),
        }
    }

    /// Compresses `data` and sync-flushes, returning this chunk's bytes. The
    /// peer can inflate the chunk completely without later data.
    ///
    /// # Errors
    /// [`DecodeError::Malformed`] if the compressor fails (it should not).
    pub(crate) fn compress(&mut self, data: &[u8]) -> Result<Vec<u8>, DecodeError> {
        const FAILED: DecodeError = DecodeError::Malformed("zlib compression failed");
        let mut out = Vec::with_capacity(data.len() + data.len() / 64 + 64);
        let start = self.inner.total_in();
        loop {
            let done = usize::try_from(self.inner.total_in() - start).map_err(|_| FAILED)?;
            let rest = data.get(done..).ok_or(FAILED)?;
            self.inner
                .compress_vec(rest, &mut out, FlushCompress::Sync)
                .map_err(|_| FAILED)?;
            let done = usize::try_from(self.inner.total_in() - start).map_err(|_| FAILED)?;
            // The flush is complete once all input is in and the compressor
            // stopped before filling the buffer.
            if done == data.len() && out.len() < out.capacity() {
                return Ok(out);
            }
            out.reserve(out.capacity().max(64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inflate::ZlibStream;

    #[test]
    fn chunks_inflate_independently_but_share_the_dictionary() {
        let mut deflate = ZlibCompressor::new(6);
        let mut inflate = ZlibStream::default();
        let text = b"the quick brown fox jumps over the lazy dog".repeat(20);
        let first = deflate.compress(&text).unwrap();
        let second = deflate.compress(&text).unwrap();
        assert!(second.len() < first.len(), "dictionary reused");
        assert_eq!(inflate.inflate_exact(&first, text.len()).unwrap(), text);
        assert_eq!(inflate.inflate_exact(&second, text.len()).unwrap(), text);
        let empty = deflate.compress(&[]).unwrap();
        assert_eq!(inflate.inflate(&empty, 0).unwrap(), b"");
    }

    #[test]
    fn large_incompressible_chunks_grow_the_buffer() {
        let data: Vec<u8> = (0..200_000u32)
            .map(|i| u8::try_from(i.wrapping_mul(2_654_435_761) >> 24).unwrap())
            .collect();
        let mut deflate = ZlibCompressor::new(0);
        let chunk = deflate.compress(&data).unwrap();
        assert_eq!(
            ZlibStream::default()
                .inflate_exact(&chunk, data.len())
                .unwrap(),
            data
        );
    }
}
