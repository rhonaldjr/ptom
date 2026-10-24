//! The `SCREEN` structure shared by the `ExtendedDesktopSize` pseudo-encoding
//! and the `SetDesktopSize` message (rfbproto "Screen Model").

use crate::DecodeError;
use crate::wire::{Reader, WriteExt};

/// Size in bytes of an encoded [`Screen`].
pub const SCREEN_LEN: usize = 16;

/// One head of a (possibly multi-monitor) desktop, in framebuffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Screen {
    /// Identifier that stays the same for the lifetime of the screen.
    pub id: u32,
    /// Left edge.
    pub x: u16,
    /// Top edge.
    pub y: u16,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// Currently unused; unknown bits must be preserved.
    pub flags: u32,
}

impl Screen {
    /// Appends the 16-byte wire encoding to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.put_u32(self.id);
        out.put_u16(self.x);
        out.put_u16(self.y);
        out.put_u16(self.width);
        out.put_u16(self.height);
        out.put_u32(self.flags);
    }

    /// Reads one screen.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] if fewer than 16 bytes remain.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            id: r.u32()?,
            x: r.u16()?,
            y: r.u16()?,
            width: r.u16()?,
            height: r.u16()?,
            flags: r.u32()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_matches_the_spec_layout() {
        let screen = Screen {
            id: 0x0102_0304,
            x: 5,
            y: 6,
            width: 1920,
            height: 1080,
            flags: 0xdead_beef,
        };
        let mut out = Vec::new();
        screen.encode(&mut out);
        assert_eq!(
            out,
            [
                1, 2, 3, 4, 0, 5, 0, 6, 0x07, 0x80, 0x04, 0x38, 0xde, 0xad, 0xbe, 0xef
            ]
        );
        assert_eq!(out.len(), SCREEN_LEN);
        assert_eq!(Screen::decode(&mut Reader::new(&out)), Ok(screen));
        assert!(
            Screen::decode(&mut Reader::new(&out[..15]))
                .unwrap_err()
                .is_incomplete()
        );
    }
}
