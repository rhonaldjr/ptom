//! Persistent zlib streams with bounded output (zlib-bomb protection).

use std::fmt;

use farsight_rfb::DecodeError;
use flate2::{Decompress, FlushDecompress, Status};

/// Output is grown in steps of at most this many bytes, so a small
/// compressed chunk never causes a huge up-front allocation.
const GROW_STEP: usize = 64 * 1024;

/// One zlib stream whose dictionary persists across rectangles (ZRLE uses
/// one per connection, Tight uses four).
pub(crate) struct ZlibStream {
    inner: Decompress,
}

impl fmt::Debug for ZlibStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZlibStream")
            .field("total_in", &self.inner.total_in())
            .field("total_out", &self.inner.total_out())
            .finish()
    }
}

impl Default for ZlibStream {
    fn default() -> Self {
        Self {
            inner: Decompress::new(true),
        }
    }
}

impl ZlibStream {
    /// Discards the dictionary and starts a fresh stream.
    pub(crate) fn reset(&mut self) {
        self.inner.reset(true);
    }

    /// Inflates the whole of `input`, which must produce at most `max` bytes.
    ///
    /// # Errors
    /// [`DecodeError::LimitExceeded`] if the output would exceed `max`, and
    /// [`DecodeError::Malformed`] for corrupt data or input left unused.
    pub(crate) fn inflate(&mut self, input: &[u8], max: usize) -> Result<Vec<u8>, DecodeError> {
        let limit = max.saturating_add(1);
        let mut out = Vec::with_capacity(limit.min(GROW_STEP));
        let mut consumed = 0usize;
        loop {
            if out.len() == out.capacity() {
                out.reserve_exact((limit - out.len().min(limit)).clamp(1, GROW_STEP));
            }
            let rest = input.get(consumed..).unwrap_or_default();
            let (in_before, out_before) = (self.inner.total_in(), self.inner.total_out());
            let status = self
                .inner
                .decompress_vec(rest, &mut out, FlushDecompress::Sync)
                .map_err(|_| DecodeError::Malformed("corrupt zlib data"))?;
            let used = self.inner.total_in() - in_before;
            let produced = self.inner.total_out() - out_before;
            consumed = consumed.saturating_add(usize::try_from(used).unwrap_or(usize::MAX));
            if out.len() > max {
                return Err(DecodeError::LimitExceeded {
                    what: "inflated data",
                    len: out.len() as u64,
                    max: max as u64,
                });
            }
            let stalled = used == 0 && produced == 0 && out.len() < out.capacity();
            if status == Status::StreamEnd || stalled {
                break;
            }
        }
        if consumed < input.len() {
            return Err(DecodeError::Malformed("unused data after zlib stream end"));
        }
        Ok(out)
    }

    /// Inflates `input`, which must produce exactly `len` bytes.
    ///
    /// # Errors
    /// As [`Self::inflate`], plus [`DecodeError::Malformed`] if fewer than
    /// `len` bytes come out.
    pub(crate) fn inflate_exact(
        &mut self,
        input: &[u8],
        len: usize,
    ) -> Result<Vec<u8>, DecodeError> {
        let out = self.inflate(input, len)?;
        if out.len() == len {
            Ok(out)
        } else {
            Err(DecodeError::Malformed(
                "zlib data shorter than the rectangle",
            ))
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use flate2::{Compress, Compression, FlushCompress};

    /// A test-only zlib compressor that keeps its dictionary between chunks,
    /// like a VNC server's per-connection stream.
    pub(crate) struct Deflater(Compress);

    impl Default for Deflater {
        fn default() -> Self {
            Self(Compress::new(Compression::default(), true))
        }
    }

    impl Deflater {
        /// Compresses `data` and sync-flushes, returning this chunk's bytes.
        pub(crate) fn chunk(&mut self, data: &[u8]) -> Vec<u8> {
            let mut out = Vec::with_capacity(data.len() * 2 + 64);
            let start = self.0.total_in();
            loop {
                let done = usize::try_from(self.0.total_in() - start).unwrap();
                self.0
                    .compress_vec(&data[done..], &mut out, FlushCompress::Sync)
                    .unwrap();
                let done = usize::try_from(self.0.total_in() - start).unwrap();
                if done == data.len() && out.len() < out.capacity() {
                    return out;
                }
                out.reserve(out.capacity());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Deflater;
    use super::*;

    #[test]
    fn stream_state_persists_across_chunks() {
        let mut deflater = Deflater::default();
        let mut stream = ZlibStream::default();
        let a = deflater.chunk(b"hello hello hello");
        let b = deflater.chunk(b"hello hello hello");
        assert_eq!(stream.inflate_exact(&a, 17).unwrap(), b"hello hello hello");
        assert_eq!(stream.inflate_exact(&b, 17).unwrap(), b"hello hello hello");
    }

    #[test]
    fn output_beyond_the_limit_is_a_bomb() {
        let chunk = Deflater::default().chunk(&vec![0u8; 100_000]);
        assert!(chunk.len() < 1000);
        assert!(matches!(
            ZlibStream::default().inflate(&chunk, 99_999),
            Err(DecodeError::LimitExceeded { .. })
        ));
        assert_eq!(
            ZlibStream::default()
                .inflate(&chunk, 100_000)
                .unwrap()
                .len(),
            100_000
        );
    }

    #[test]
    fn short_output_and_garbage_are_malformed() {
        let chunk = Deflater::default().chunk(b"abc");
        assert!(matches!(
            ZlibStream::default().inflate_exact(&chunk, 4),
            Err(DecodeError::Malformed(_))
        ));
        assert!(matches!(
            ZlibStream::default().inflate(&[0xff; 16], 100),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn reset_starts_a_new_stream() {
        let mut stream = ZlibStream::default();
        let mut deflater = Deflater::default();
        stream.inflate_exact(&deflater.chunk(b"one"), 3).unwrap();
        stream.reset();
        let fresh = Deflater::default().chunk(b"two");
        assert_eq!(stream.inflate_exact(&fresh, 3).unwrap(), b"two");
    }
}
