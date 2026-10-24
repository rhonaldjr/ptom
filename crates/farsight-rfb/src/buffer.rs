//! An input buffer that consumes from the front without shifting on every read.

/// Bytes received but not yet consumed.
#[derive(Debug, Default)]
pub(crate) struct InputBuffer {
    data: Vec<u8>,
    start: usize,
}

/// Compact once this many consumed bytes have accumulated at the front.
const COMPACT_THRESHOLD: usize = 64 * 1024;

/// Most unread input a connection may hold: comfortably above the largest
/// legitimate message (clipboard text is capped by `Limits`), so a peer that
/// streams data the state machine isn't consuming can't exhaust memory.
pub(crate) const MAX_UNREAD: usize = 32 * 1024 * 1024;

impl InputBuffer {
    /// Appends `bytes`. Returns false (and drops them) if that would exceed
    /// [`MAX_UNREAD`].
    #[must_use]
    pub(crate) fn extend(&mut self, bytes: &[u8]) -> bool {
        if self.unread().len().saturating_add(bytes.len()) > MAX_UNREAD {
            return false;
        }
        self.data.extend_from_slice(bytes);
        true
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
    fn refuses_input_beyond_the_cap() {
        let mut buf = InputBuffer::default();
        assert!(buf.extend(&vec![0u8; MAX_UNREAD]));
        assert!(!buf.extend(&[1]));
        assert_eq!(buf.unread().len(), MAX_UNREAD);
    }

    #[test]
    fn consumes_and_compacts() {
        let mut buf = InputBuffer::default();
        assert!(buf.extend(&vec![7u8; COMPACT_THRESHOLD + 10]));
        buf.consume(COMPACT_THRESHOLD);
        assert_eq!(buf.unread().len(), 10);
        assert!(buf.extend(&[1, 2]));
        assert_eq!(buf.unread().len(), 12);
        buf.consume(12);
        assert_eq!(buf.unread().len(), 0);
        buf.consume(5);
        assert_eq!(buf.unread().len(), 0);
    }
}
