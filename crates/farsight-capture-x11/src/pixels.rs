//! Converting X11 `ZPixmap` image data to `0x00RRGGBB` pixels, and cursor
//! images to straight RGBA.

use crate::CaptureError;

/// One colour channel inside a pixel value, described by its visual mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Channel {
    shift: u32,
    bits: u32,
}

impl Channel {
    fn from_mask(mask: u32) -> Option<Self> {
        if mask == 0 {
            return None;
        }
        let shift = mask.trailing_zeros();
        let bits = (mask >> shift).trailing_ones();
        // The mask must be one contiguous run of set bits.
        if (mask >> shift) >> bits != 0 {
            return None;
        }
        Some(Self { shift, bits })
    }

    /// The channel's value from `pixel`, scaled to 8 bits.
    fn extract(self, pixel: u32) -> u32 {
        let max = (1u64 << self.bits) - 1;
        let value = u64::from(pixel >> self.shift) & max;
        let scaled = if self.bits >= 8 {
            value >> (self.bits - 8)
        } else {
            (value * 255 + max / 2) / max
        };
        u32::try_from(scaled).unwrap_or(0xFF)
    }
}

/// How the X server lays out pixels for the captured visual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PixelLayout {
    bytes_per_pixel: usize,
    scanline_pad_bits: usize,
    big_endian: bool,
    red: Channel,
    green: Channel,
    blue: Channel,
}

/// The raw description of a visual and its pixmap format, as read from the X setup.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VisualFormat {
    pub bits_per_pixel: u8,
    pub scanline_pad: u8,
    pub big_endian: bool,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
}

impl PixelLayout {
    /// Validates a visual's masks and pixmap format.
    ///
    /// # Errors
    /// [`CaptureError::UnsupportedVisual`] for masks that are empty or not
    /// contiguous, or for a pixel size this converter does not handle.
    pub(crate) fn new(format: VisualFormat) -> Result<Self, CaptureError> {
        let unsupported = |why: &str| CaptureError::UnsupportedVisual(why.to_owned());
        let bytes_per_pixel = match format.bits_per_pixel {
            8 => 1,
            16 => 2,
            24 => 3,
            32 => 4,
            other => return Err(unsupported(&format!("{other} bits per pixel"))),
        };
        if !matches!(format.scanline_pad, 8 | 16 | 32) {
            return Err(unsupported("scanline pad is not 8, 16 or 32"));
        }
        let channel = |mask| Channel::from_mask(mask).ok_or_else(|| unsupported("bad colour mask"));
        Ok(Self {
            bytes_per_pixel,
            scanline_pad_bits: usize::from(format.scanline_pad),
            big_endian: format.big_endian,
            red: channel(format.red_mask)?,
            green: channel(format.green_mask)?,
            blue: channel(format.blue_mask)?,
        })
    }

    /// Bytes per image row of `width` pixels, including scanline padding.
    pub(crate) fn stride(&self, width: u16) -> usize {
        let bits = usize::from(width) * self.bytes_per_pixel * 8;
        bits.div_ceil(self.scanline_pad_bits) * self.scanline_pad_bits / 8
    }

    fn is_native_xrgb(&self) -> bool {
        self.bytes_per_pixel == 4
            && !self.big_endian
            && self.red == Channel { shift: 16, bits: 8 }
            && self.green == Channel { shift: 8, bits: 8 }
            && self.blue == Channel { shift: 0, bits: 8 }
    }

    fn raw_value(&self, bytes: &[u8]) -> u32 {
        let fold = |acc: u32, byte: &u8| (acc << 8) | u32::from(*byte);
        if self.big_endian {
            bytes.iter().fold(0, fold)
        } else {
            bytes.iter().rev().fold(0, fold)
        }
    }

    fn to_rgb(&self, raw: u32) -> u32 {
        (self.red.extract(raw) << 16) | (self.green.extract(raw) << 8) | self.blue.extract(raw)
    }

    /// Converts a `width` × `height` image whose rows are `stride` bytes
    /// apart into `0x00RRGGBB` pixels. Missing data reads as black.
    pub(crate) fn convert(&self, data: &[u8], width: u16, height: u16, stride: usize) -> Vec<u32> {
        let width = usize::from(width);
        let mut out = Vec::with_capacity(width * usize::from(height));
        let row_bytes = width * self.bytes_per_pixel;
        let native = self.is_native_xrgb();
        for row in 0..usize::from(height) {
            let start = row * stride;
            let rest = data.get(start..).unwrap_or_default();
            let line = rest.get(..row_bytes.min(rest.len())).unwrap_or_default();
            let before = out.len();
            for chunk in line.chunks_exact(self.bytes_per_pixel) {
                let pixel = if native {
                    self.raw_value(chunk) & 0x00FF_FFFF
                } else {
                    self.to_rgb(self.raw_value(chunk))
                };
                out.push(pixel);
            }
            out.resize(before + width, 0);
        }
        out
    }
}

/// Converts XFixes cursor pixels (premultiplied `0xAARRGGBB`) to straight
/// (non-premultiplied) RGBA bytes.
pub(crate) fn cursor_to_rgba(pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * 4);
    for &pixel in pixels {
        let alpha = pixel >> 24;
        let unpremultiply = |shift: u32| -> u8 {
            if alpha == 0 {
                return 0;
            }
            let channel = (pixel >> shift) & 0xFF;
            let straight = (channel * 255 + alpha / 2) / alpha;
            u8::try_from(straight.min(255)).unwrap_or(u8::MAX)
        };
        out.extend_from_slice(&[
            unpremultiply(16),
            unpremultiply(8),
            unpremultiply(0),
            u8::try_from(alpha).unwrap_or(u8::MAX),
        ]);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unreadable_literal)]
mod tests {
    use super::*;

    fn format(bits_per_pixel: u8, big_endian: bool, masks: (u32, u32, u32)) -> VisualFormat {
        VisualFormat {
            bits_per_pixel,
            scanline_pad: 32,
            big_endian,
            red_mask: masks.0,
            green_mask: masks.1,
            blue_mask: masks.2,
        }
    }

    #[test]
    fn xrgb32_little_endian_is_copied() {
        let layout = PixelLayout::new(format(32, false, (0xFF0000, 0xFF00, 0xFF))).unwrap();
        let data = [0x33, 0x22, 0x11, 0xAA, 0x01, 0x02, 0x03, 0x00];
        assert_eq!(layout.convert(&data, 2, 1, 8), [0x112233, 0x030201]);
    }

    #[test]
    fn bgr32_big_endian_uses_masks() {
        let layout = PixelLayout::new(format(32, true, (0xFF, 0xFF00, 0xFF0000))).unwrap();
        // Big-endian value 0x00_33_22_11: red is the low byte.
        assert_eq!(
            layout.convert(&[0x00, 0x33, 0x22, 0x11], 1, 1, 4),
            [0x112233]
        );
    }

    #[test]
    fn rgb565_expands_to_full_range() {
        let layout = PixelLayout::new(format(16, false, (0xF800, 0x07E0, 0x001F))).unwrap();
        let white = 0xFFFFu16.to_le_bytes();
        let red = 0xF800u16.to_le_bytes();
        let data = [white[0], white[1], red[0], red[1]];
        assert_eq!(layout.convert(&data, 2, 1, 4), [0xFFFFFF, 0xFF0000]);
    }

    #[test]
    fn padded_rows_are_skipped() {
        let layout = PixelLayout::new(format(24, false, (0xFF0000, 0xFF00, 0xFF))).unwrap();
        assert_eq!(layout.stride(1), 4);
        let data = [0x03, 0x02, 0x01, 0xEE, 0x06, 0x05, 0x04, 0xEE];
        assert_eq!(layout.convert(&data, 1, 2, 4), [0x010203, 0x040506]);
    }

    #[test]
    fn short_data_reads_as_black() {
        let layout = PixelLayout::new(format(32, false, (0xFF0000, 0xFF00, 0xFF))).unwrap();
        assert_eq!(layout.convert(&[1, 2, 3, 4], 2, 2, 8), [0x030201, 0, 0, 0]);
    }

    #[test]
    fn rejects_bad_masks_and_sizes() {
        assert!(PixelLayout::new(format(32, false, (0, 0xFF00, 0xFF))).is_err());
        assert!(PixelLayout::new(format(32, false, (0xF0F000, 0xFF00, 0xFF))).is_err());
        assert!(PixelLayout::new(format(4, false, (0xF, 0xF, 0xF))).is_err());
    }

    #[test]
    fn cursor_is_unpremultiplied() {
        let rgba = cursor_to_rgba(&[0xFF10_2030, 0x8040_0000, 0x0012_3456]);
        assert_eq!(&rgba[0..4], &[0x10, 0x20, 0x30, 0xFF]);
        assert_eq!(&rgba[4..8], &[0x80, 0, 0, 0x80]);
        assert_eq!(&rgba[8..12], &[0, 0, 0, 0]);
    }
}
