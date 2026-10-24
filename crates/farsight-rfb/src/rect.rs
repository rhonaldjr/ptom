//! Screen rectangles.

/// An axis-aligned rectangle in framebuffer coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    /// Left edge.
    pub x: u16,
    /// Top edge.
    pub y: u16,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
}

impl Rect {
    /// Creates a rectangle.
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Number of pixels covered.
    #[must_use]
    pub fn area(&self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }

    /// Whether the rectangle has no pixels.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Exclusive right edge, computed without overflow.
    #[must_use]
    pub fn right(&self) -> u32 {
        u32::from(self.x) + u32::from(self.width)
    }

    /// Exclusive bottom edge, computed without overflow.
    #[must_use]
    pub fn bottom(&self) -> u32 {
        u32::from(self.y) + u32::from(self.height)
    }

    /// Whether the rectangle lies entirely within a `width` × `height` area.
    #[must_use]
    pub fn fits_within(&self, width: u16, height: u16) -> bool {
        self.right() <= u32::from(width) && self.bottom() <= u32::from(height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_within_handles_edges_without_overflow() {
        assert!(Rect::new(0, 0, 10, 10).fits_within(10, 10));
        assert!(!Rect::new(1, 0, 10, 10).fits_within(10, 10));
        assert!(!Rect::new(u16::MAX, u16::MAX, u16::MAX, u16::MAX).fits_within(u16::MAX, u16::MAX));
        assert!(Rect::new(5, 5, 0, 0).fits_within(10, 10));
    }
}
