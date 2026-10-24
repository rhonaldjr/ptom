//! The viewer's copy of the remote screen.

use farsight_rfb::{DecodeError, Rect};

/// A width × height grid of `0x00RRGGBB` pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    pixels: Vec<u32>,
}

impl Framebuffer {
    /// Creates a black framebuffer.
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height)],
        }
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u16 {
        self.height
    }

    /// All pixels, row-major.
    #[must_use]
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// Resizes, clearing to black.
    pub fn resize(&mut self, width: u16, height: u16) {
        *self = Self::new(width, height);
    }

    /// The pixel at (`x`, `y`), if inside.
    #[must_use]
    pub fn get(&self, x: u16, y: u16) -> Option<u32> {
        if x >= self.width {
            return None;
        }
        self.pixels.get(self.index(x, y)).copied()
    }

    fn index(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.width) + usize::from(x)
    }

    fn check(&self, rect: Rect) -> Result<(), DecodeError> {
        if rect.fits_within(self.width, self.height) {
            Ok(())
        } else {
            Err(DecodeError::RectOutOfBounds)
        }
    }

    fn row_mut(&mut self, rect: Rect, row: u16) -> Result<&mut [u32], DecodeError> {
        let start = self.index(rect.x, rect.y + row);
        self.pixels
            .get_mut(start..start + usize::from(rect.width))
            .ok_or(DecodeError::RectOutOfBounds)
    }

    /// Fills `rect` with one colour.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the framebuffer.
    pub fn fill(&mut self, rect: Rect, rgb: u32) -> Result<(), DecodeError> {
        self.check(rect)?;
        for row in 0..rect.height {
            self.row_mut(rect, row)?.fill(rgb);
        }
        Ok(())
    }

    /// Writes `rect` from row-major pixels produced by `pixel(index)`.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the framebuffer.
    pub fn write(
        &mut self,
        rect: Rect,
        mut pixel: impl FnMut(usize) -> u32,
    ) -> Result<(), DecodeError> {
        self.check(rect)?;
        let mut i = 0;
        for row in 0..rect.height {
            for slot in self.row_mut(rect, row)? {
                *slot = pixel(i);
                i += 1;
            }
        }
        Ok(())
    }

    /// Copies the `dst`-sized area whose top-left is (`src_x`, `src_y`) to `dst`.
    /// Overlapping source and destination are handled correctly.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if either area is not inside the framebuffer.
    pub fn copy(&mut self, src_x: u16, src_y: u16, dst: Rect) -> Result<(), DecodeError> {
        let src = Rect::new(src_x, src_y, dst.width, dst.height);
        self.check(src)?;
        self.check(dst)?;
        let rows: Box<dyn Iterator<Item = u16>> = if src_y < dst.y {
            Box::new((0..dst.height).rev())
        } else {
            Box::new(0..dst.height)
        };
        let width = usize::from(dst.width);
        for row in rows {
            let from = self.index(src.x, src.y + row);
            let to = self.index(dst.x, dst.y + row);
            if from + width > self.pixels.len() || to + width > self.pixels.len() {
                return Err(DecodeError::RectOutOfBounds);
            }
            self.pixels.copy_within(from..from + width, to);
        }
        Ok(())
    }

    /// Reads `rect` as row-major pixels.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the framebuffer.
    pub fn read(&self, rect: Rect) -> Result<Vec<u32>, DecodeError> {
        self.check(rect)?;
        let mut out = Vec::with_capacity(rect.area());
        for row in 0..rect.height {
            let start = self.index(rect.x, rect.y + row);
            out.extend_from_slice(
                self.pixels
                    .get(start..start + usize::from(rect.width))
                    .ok_or(DecodeError::RectOutOfBounds)?,
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_read() {
        let mut fb = Framebuffer::new(4, 3);
        fb.fill(Rect::new(1, 1, 2, 2), 7).unwrap();
        assert_eq!(fb.read(Rect::new(0, 1, 4, 1)).unwrap(), [0, 7, 7, 0]);
        assert_eq!(
            fb.fill(Rect::new(3, 0, 2, 1), 1),
            Err(DecodeError::RectOutOfBounds)
        );
    }

    #[test]
    fn overlapping_copy_downwards_and_upwards() {
        let mut fb = Framebuffer::new(1, 4);
        fb.write(Rect::new(0, 0, 1, 4), |i| u32::try_from(i).unwrap())
            .unwrap();
        fb.copy(0, 0, Rect::new(0, 1, 1, 3)).unwrap();
        assert_eq!(fb.pixels(), [0, 0, 1, 2]);
        fb.copy(0, 1, Rect::new(0, 0, 1, 3)).unwrap();
        assert_eq!(fb.pixels(), [0, 1, 2, 2]);
    }

    #[test]
    fn copy_rejects_out_of_bounds_source() {
        let mut fb = Framebuffer::new(4, 4);
        assert_eq!(
            fb.copy(3, 3, Rect::new(0, 0, 2, 2)),
            Err(DecodeError::RectOutOfBounds)
        );
    }
}
