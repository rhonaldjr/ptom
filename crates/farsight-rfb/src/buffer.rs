//! An input buffer that consumes from the front without shifting on every read.

/// Bytes received but not yet consumed.
#[derive(Debug, Default)]
pub(crate) struct InputBuffer {
    data: Vec<u8>,
    start: usize,
}

/// Compact once this many consumed bytes have accumulated at the front.
const COMPACT_THRESHOLD: usize = 64 * 1024;

impl InputBuffer {
    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        self.data.extend_from_slice(bytes);
    }

    pub(crate) fn unread(&self) -> &[u8] {
        self.data.get(self.start..).unwrap_or_default()
    }

    pub(crate) fn consume(&mut self, count: usize) {
        self.start = (self.start + count).min(self.data.len());
        if self.start == self.data.len() {
            self.data.clear();
            self.start = 0;
        } else if self.start >= COMPACT_THRESHOLD && self.start * 2 >= self.data.len() {
            self.data.drain(..self.start);
            self.start = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_and_compacts() {
        let mut buf = InputBuffer::default();
        buf.extend(&vec![7u8; COMPACT_THRESHOLD + 10]);
        buf.consume(COMPACT_THRESHOLD);
        assert_eq!(buf.unread().len(), 10);
        buf.extend(&[1, 2]);
        assert_eq!(buf.unread().len(), 12);
        buf.consume(12);
        assert_eq!(buf.unread().len(), 0);
        buf.consume(5);
        assert_eq!(buf.unread().len(), 0);
    }
}
