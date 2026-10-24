//! Converting captured buffers into `0x00RRGGBB` framebuffers and RGBA cursor images.
//!
//! PipeWire names formats by byte order in memory: `BGRx` is the bytes B, G, R, padding.

use farsight_codec::Framebuffer;
use farsight_rfb::Rect;

/// Bytes per pixel of every format we negotiate.
pub const BYTES_PER_PIXEL: usize = 4;

/// The 32-bit packed formats we accept from the compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Bytes B, G, R, padding.
    Bgrx,
    /// Bytes R, G, B, padding.
    Rgbx,
    /// Bytes B, G, R, A.
    Bgra,
    /// Bytes R, G, B, A.
    Rgba,
}

impl PixelFormat {
    /// Red, green and blue of one pixel's four bytes.
    #[must_use]
    pub fn rgb(self, px: [u8; 4]) -> (u8, u8, u8) {
        let [a, b, c, _] = px;
        match self {
            Self::Bgrx | Self::Bgra => (c, b, a),
            Self::Rgbx | Self::Rgba => (a, b, c),
        }
    }

    /// Alpha of one pixel's four bytes (opaque for the padded formats).
    #[must_use]
    pub fn alpha(self, px: [u8; 4]) -> u8 {
        let [_, _, _, alpha] = px;
        match self {
            Self::Bgra | Self::Rgba => alpha,
            Self::Bgrx | Self::Rgbx => u8::MAX,
        }
    }

    /// The pixel as `0x00RRGGBB`.
    #[must_use]
    pub fn to_rgb32(self, px: [u8; 4]) -> u32 {
        let (r, g, b) = self.rgb(px);
        u32::from_be_bytes([0, r, g, b])
    }
}

/// Why a buffer could not be converted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConvertError {
    /// Width or height does not fit the RFB 16-bit range.
    #[error("frame is too large ({width}x{height})")]
    TooLarge {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// The stride is shorter than one row of pixels.
    #[error("stride {stride} is shorter than a {width}-pixel row")]
    BadStride {
        /// Bytes per row as reported.
        stride: usize,
        /// Width in pixels.
        width: u32,
    },
    /// The buffer holds fewer bytes than the geometry needs.
    #[error("buffer holds {len} bytes, needs {needed}")]
    Truncated {
        /// Bytes available.
        len: usize,
        /// Bytes needed.
        needed: usize,
    },
}

/// The geometry of one image in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Pixel format.
    pub format: PixelFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes from the start of one row to the next (may include padding).
    pub stride: usize,
}

impl Layout {
    fn dimensions(&self) -> Result<(u16, u16), ConvertError> {
        let too_large = ConvertError::TooLarge {
            width: self.width,
            height: self.height,
        };
        let width = u16::try_from(self.width).map_err(|_| too_large)?;
        let height = u16::try_from(self.height).map_err(|_| too_large)?;
        Ok((width, height))
    }

    fn row_bytes(&self) -> usize {
        // Fits: width is at most u16::MAX after `dimensions`.
        usize::try_from(self.width).unwrap_or(usize::MAX) * BYTES_PER_PIXEL
    }

    /// Checks the stride and that `len` bytes hold every row, returning the row length in bytes.
    fn check(&self, len: usize) -> Result<usize, ConvertError> {
        let row = self.row_bytes();
        if self.stride < row {
            return Err(ConvertError::BadStride {
                stride: self.stride,
                width: self.width,
            });
        }
        let rows = usize::try_from(self.height).unwrap_or(usize::MAX);
        let needed = match rows {
            0 => 0,
            n => self
                .stride
                .checked_mul(n - 1)
                .and_then(|v| v.checked_add(row))
                .unwrap_or(usize::MAX),
        };
        if len < needed {
            return Err(ConvertError::Truncated { len, needed });
        }
        Ok(row)
    }

    /// The rows of `data`, each exactly one row of pixels long (stride padding removed).
    fn rows<'a>(
        &self,
        data: &'a [u8],
    ) -> Result<impl Iterator<Item = &'a [u8]> + 'a, ConvertError> {
        let row = self.check(data.len())?;
        let rows = usize::try_from(self.height).unwrap_or(0);
        let stride = self.stride;
        Ok((0..rows).filter_map(move |y| {
            let start = y * stride;
            data.get(start..start + row)
        }))
    }
}

fn pixels_of(row: &[u8]) -> impl Iterator<Item = [u8; 4]> + '_ {
    row.as_chunks::<BYTES_PER_PIXEL>().0.iter().copied()
}

/// Converts a captured image into a framebuffer, honouring the stride.
///
/// # Errors
/// [`ConvertError`] if the geometry is out of range or `data` is too short for it.
pub fn to_framebuffer(layout: Layout, data: &[u8]) -> Result<Framebuffer, ConvertError> {
    let (width, height) = layout.dimensions()?;
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height));
    for row in layout.rows(data)? {
        pixels.extend(pixels_of(row).map(|px| layout.format.to_rgb32(px)));
    }
    let mut framebuffer = Framebuffer::new(width, height);
    framebuffer
        .write(Rect::new(0, 0, width, height), |i| {
            pixels.get(i).copied().unwrap_or(0)
        })
        .map_err(|_| ConvertError::Truncated {
            len: data.len(),
            needed: data.len(),
        })?;
    Ok(framebuffer)
}

/// Converts a cursor bitmap into tightly packed RGBA bytes, honouring the stride.
///
/// # Errors
/// [`ConvertError`] if the geometry is out of range or `data` is too short for it.
pub fn to_rgba(layout: Layout, data: &[u8]) -> Result<Vec<u8>, ConvertError> {
    layout.dimensions()?;
    let mut out =
        Vec::with_capacity(layout.row_bytes() * usize::try_from(layout.height).unwrap_or(0));
    for row in layout.rows(data)? {
        for px in pixels_of(row) {
            let (r, g, b) = layout.format.rgb(px);
            out.extend_from_slice(&[r, g, b, layout.format.alpha(px)]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const FORMATS: [PixelFormat; 4] = [
        PixelFormat::Bgrx,
        PixelFormat::Rgbx,
        PixelFormat::Bgra,
        PixelFormat::Rgba,
    ];

    fn encode(format: PixelFormat, rgb: u32, alpha: u8) -> [u8; 4] {
        let [_, r, g, b] = rgb.to_be_bytes();
        match format {
            PixelFormat::Bgrx | PixelFormat::Bgra => [b, g, r, alpha],
            PixelFormat::Rgbx | PixelFormat::Rgba => [r, g, b, alpha],
        }
    }

    #[test]
    fn byte_orders_map_to_rgb32() {
        let px = [0x11, 0x22, 0x33, 0x44];
        assert_eq!(PixelFormat::Bgrx.to_rgb32(px), 0x0033_2211);
        assert_eq!(PixelFormat::Bgra.to_rgb32(px), 0x0033_2211);
        assert_eq!(PixelFormat::Rgbx.to_rgb32(px), 0x0011_2233);
        assert_eq!(PixelFormat::Rgba.to_rgb32(px), 0x0011_2233);
        assert_eq!(PixelFormat::Bgrx.alpha(px), 0xff);
        assert_eq!(PixelFormat::Rgba.alpha(px), 0x44);
    }

    #[test]
    fn stride_padding_is_skipped() {
        // 2x2 BGRx with 4 bytes of padding per row; padding is 0xEE.
        let data = [
            1, 2, 3, 0, 4, 5, 6, 0, 0xEE, 0xEE, 0xEE, 0xEE, //
            7, 8, 9, 0, 10, 11, 12, 0,
        ];
        let layout = Layout {
            format: PixelFormat::Bgrx,
            width: 2,
            height: 2,
            stride: 12,
        };
        let fb = to_framebuffer(layout, &data).unwrap();
        assert_eq!(
            fb.pixels(),
            [0x0003_0201, 0x0006_0504, 0x0009_0807, 0x000c_0b0a]
        );
    }

    #[test]
    fn last_row_needs_no_padding() {
        let layout = Layout {
            format: PixelFormat::Rgbx,
            width: 1,
            height: 2,
            stride: 16,
        };
        assert!(to_framebuffer(layout, &[0; 20]).is_ok());
        assert_eq!(
            to_framebuffer(layout, &[0; 19]),
            Err(ConvertError::Truncated {
                len: 19,
                needed: 20
            })
        );
    }

    #[test]
    fn rejects_bad_geometry() {
        let short_stride = Layout {
            format: PixelFormat::Rgba,
            width: 4,
            height: 1,
            stride: 15,
        };
        assert!(matches!(
            to_framebuffer(short_stride, &[0; 64]),
            Err(ConvertError::BadStride { .. })
        ));
        let huge = Layout {
            format: PixelFormat::Rgba,
            width: 70_000,
            height: 1,
            stride: 280_000,
        };
        assert!(matches!(
            to_framebuffer(huge, &[]),
            Err(ConvertError::TooLarge { .. })
        ));
        let overflow = Layout {
            format: PixelFormat::Rgba,
            width: 1,
            height: 65_535,
            stride: usize::MAX,
        };
        assert!(matches!(
            to_framebuffer(overflow, &[0; 8]),
            Err(ConvertError::Truncated { .. })
        ));
    }

    #[test]
    fn empty_image_converts() {
        let layout = Layout {
            format: PixelFormat::Bgrx,
            width: 0,
            height: 0,
            stride: 0,
        };
        assert_eq!(to_framebuffer(layout, &[]).unwrap().pixels(), &[] as &[u32]);
    }

    #[test]
    fn cursor_rgba_keeps_alpha() {
        let layout = Layout {
            format: PixelFormat::Bgra,
            width: 1,
            height: 1,
            stride: 4,
        };
        assert_eq!(to_rgba(layout, &[1, 2, 3, 4]).unwrap(), [3, 2, 1, 4]);
    }

    proptest! {
        #[test]
        fn converts_any_stride(
            format_index in 0usize..4,
            width in 0u32..24,
            height in 0u32..24,
            padding in 0usize..17,
            seed in any::<u32>(),
        ) {
            let format = FORMATS[format_index];
            let stride = width as usize * BYTES_PER_PIXEL + padding;
            let expected: Vec<u32> = (0..width * height)
                .map(|i| i.wrapping_mul(2_654_435_761).wrapping_add(seed) & 0x00ff_ffff)
                .collect();
            let mut data = vec![0xAB; stride * height as usize];
            for (i, rgb) in expected.iter().enumerate() {
                let (x, y) = (i % width as usize, i / width as usize);
                let at = y * stride + x * BYTES_PER_PIXEL;
                data[at..at + 4].copy_from_slice(&encode(format, *rgb, 0x80));
            }
            let layout = Layout { format, width, height, stride };
            let fb = to_framebuffer(layout, &data).unwrap();
            prop_assert_eq!(fb.pixels(), expected.as_slice());
            let rgba = to_rgba(layout, &data).unwrap();
            prop_assert_eq!(rgba.len(), expected.len() * 4);
        }

        #[test]
        fn never_panics_on_arbitrary_input(
            width in 0u32..100_000,
            height in 0u32..100,
            stride in 0usize..1_000,
            data in proptest::collection::vec(any::<u8>(), 0..512),
        ) {
            let layout = Layout { format: PixelFormat::Bgra, width, height, stride };
            let _ = to_framebuffer(layout, &data);
            let _ = to_rgba(layout, &data);
        }
    }
}
