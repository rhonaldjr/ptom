//! The 3-byte pixel forms used inside compressed encodings: ZRLE's `CPIXEL`
//! and Tight's `TPIXEL` (rfbproto "ZRLE Encoding" and "Tight Encoding").

use farsight_rfb::PixelFormat;

use crate::PixelConverter;

/// Wire layout of one compact pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Same as `PIXEL`.
    Pixel,
    /// Tight: three bytes, red then green then blue.
    Rgb,
    /// ZRLE: the three least significant bytes of the 32-bit pixel.
    Low3,
    /// ZRLE: the three most significant bytes of the 32-bit pixel.
    High3,
}

/// Reads compact pixels for one pixel format.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CompactPixel<'a> {
    converter: &'a PixelConverter,
    layout: Layout,
}

impl<'a> CompactPixel<'a> {
    /// ZRLE `CPIXEL`: 3 bytes when the format is 32 bpp true colour, depth
    /// ≤ 24, and every colour bit lies in the low (preferred) or high 3 bytes.
    pub(crate) fn cpixel(converter: &'a PixelConverter) -> Self {
        let f = converter.format();
        let layout = match colour_mask(&f) {
            Some(mask) if f.true_colour && f.bits_per_pixel == 32 && f.depth <= 24 => {
                if mask <= 0x00ff_ffff {
                    Layout::Low3
                } else if mask.trailing_zeros() >= 8 && mask <= 0xffff_ffff {
                    Layout::High3
                } else {
                    Layout::Pixel
                }
            }
            _ => Layout::Pixel,
        };
        Self { converter, layout }
    }

    /// Tight `TPIXEL`: 3 bytes R, G, B when the format is 32 bpp true colour,
    /// depth 24, with exactly 8 bits per channel.
    pub(crate) fn tpixel(converter: &'a PixelConverter) -> Self {
        let f = converter.format();
        let rgb888 = f.true_colour
            && f.bits_per_pixel == 32
            && f.depth == 24
            && [f.red_max, f.green_max, f.blue_max] == [255; 3];
        Self {
            converter,
            layout: if rgb888 { Layout::Rgb } else { Layout::Pixel },
        }
    }

    /// Bytes per compact pixel.
    pub(crate) fn len(&self) -> usize {
        match self.layout {
            Layout::Pixel => self.converter.bytes_per_pixel(),
            Layout::Rgb | Layout::Low3 | Layout::High3 => 3,
        }
    }

    /// Whether pixels are the 3-byte R, G, B form (Tight only).
    pub(crate) fn is_rgb(&self) -> bool {
        self.layout == Layout::Rgb
    }

    /// The converter for full `PIXEL` values.
    pub(crate) fn converter(&self) -> &'a PixelConverter {
        self.converter
    }

    /// Converts the compact pixel at the start of `bytes` to `0x00RRGGBB`.
    pub(crate) fn rgb(&self, bytes: &[u8]) -> u32 {
        let three = || -> [u32; 3] {
            let mut out = [0u32; 3];
            for (slot, byte) in out.iter_mut().zip(bytes) {
                *slot = u32::from(*byte);
            }
            out
        };
        match self.layout {
            Layout::Pixel => self.converter.decode(bytes),
            Layout::Rgb => {
                let [r, g, b] = three();
                (r << 16) | (g << 8) | b
            }
            Layout::Low3 | Layout::High3 => {
                let [a, b, c] = three();
                let value = if self.converter.format().big_endian {
                    (a << 16) | (b << 8) | c
                } else {
                    (c << 16) | (b << 8) | a
                };
                let raw = if self.layout == Layout::High3 {
                    value << 8
                } else {
                    value
                };
                self.converter.to_rgb(raw)
            }
        }
    }
}

/// All bits used by the red, green and blue channels, or `None` if a shift
/// puts a channel outside 64 bits.
fn colour_mask(f: &PixelFormat) -> Option<u64> {
    let channel = |max: u16, shift: u8| u64::from(max).checked_shl(u32::from(shift));
    Some(
        channel(f.red_max, f.red_shift)?
            | channel(f.green_max, f.green_shift)?
            | channel(f.blue_max, f.blue_shift)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(big_endian: bool, shifts: [u8; 3], depth: u8) -> PixelFormat {
        PixelFormat {
            big_endian,
            depth,
            red_shift: shifts[0],
            green_shift: shifts[1],
            blue_shift: shifts[2],
            ..PixelFormat::RGB888
        }
    }

    #[test]
    fn cpixel_uses_the_low_three_bytes_when_they_hold_every_colour_bit() {
        let little = PixelConverter::new(format(false, [16, 8, 0], 24));
        let c = CompactPixel::cpixel(&little);
        assert_eq!(c.len(), 3);
        // Little-endian 0x00112233 is 33 22 11 00 on the wire; CPIXEL drops the 00.
        assert_eq!(c.rgb(&[0x33, 0x22, 0x11]), 0x0011_2233);

        let big = PixelConverter::new(format(true, [16, 8, 0], 24));
        let c = CompactPixel::cpixel(&big);
        // Big-endian 0x00112233 is 00 11 22 33; CPIXEL drops the leading 00.
        assert_eq!(c.rgb(&[0x11, 0x22, 0x33]), 0x0011_2233);
    }

    #[test]
    fn cpixel_uses_the_high_three_bytes_when_colours_are_shifted_up() {
        let little = PixelConverter::new(format(false, [24, 16, 8], 24));
        let c = CompactPixel::cpixel(&little);
        assert_eq!(c.len(), 3);
        // 0x11223300 little-endian is 00 33 22 11; CPIXEL drops the 00.
        assert_eq!(c.rgb(&[0x33, 0x22, 0x11]), 0x0011_2233);

        let big = PixelConverter::new(format(true, [24, 16, 8], 24));
        // 0x11223300 big-endian is 11 22 33 00; CPIXEL drops the trailing 00.
        assert_eq!(
            CompactPixel::cpixel(&big).rgb(&[0x11, 0x22, 0x33]),
            0x0011_2233
        );
    }

    #[test]
    fn cpixel_is_three_bytes_for_shallow_32_bit_formats_and_full_otherwise() {
        let rgb565_in_32 = PixelConverter::new(PixelFormat {
            depth: 16,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..PixelFormat::RGB888
        });
        assert_eq!(CompactPixel::cpixel(&rgb565_in_32).len(), 3);
        let deep = PixelConverter::new(format(false, [16, 8, 0], 32));
        assert_eq!(CompactPixel::cpixel(&deep).len(), 4);
        let straddling = PixelConverter::new(format(false, [20, 8, 0], 24));
        assert_eq!(CompactPixel::cpixel(&straddling).len(), 4);
        let sixteen = PixelConverter::new(PixelFormat {
            bits_per_pixel: 16,
            depth: 16,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..PixelFormat::RGB888
        });
        assert_eq!(CompactPixel::cpixel(&sixteen).len(), 2);
    }

    #[test]
    fn tpixel_is_rgb_bytes_only_for_8_bit_channels_at_depth_24() {
        for shifts in [[16, 8, 0], [0, 8, 16], [24, 16, 8]] {
            for big_endian in [false, true] {
                let conv = PixelConverter::new(format(big_endian, shifts, 24));
                let t = CompactPixel::tpixel(&conv);
                assert_eq!(t.len(), 3);
                assert!(t.is_rgb());
                assert_eq!(t.rgb(&[0x11, 0x22, 0x33]), 0x0011_2233);
            }
        }
        let deep = PixelConverter::new(format(false, [16, 8, 0], 32));
        assert_eq!(CompactPixel::tpixel(&deep).len(), 4);
        assert_eq!(
            CompactPixel::tpixel(&deep).rgb(&[0x33, 0x22, 0x11, 0]),
            0x0011_2233
        );
    }
}
