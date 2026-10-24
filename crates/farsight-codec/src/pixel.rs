//! Conversion between wire pixel formats and the framebuffer's native
//! `0x00RRGGBB` representation.

use farsight_rfb::PixelFormat;

/// Converts pixels between a negotiated [`PixelFormat`] and `0x00RRGGBB`.
#[derive(Debug, Clone)]
pub struct PixelConverter {
    format: PixelFormat,
    bytes_per_pixel: usize,
    colour_map: Vec<u32>,
}

impl PixelConverter {
    /// Creates a converter for `format` (already validated).
    #[must_use]
    pub fn new(format: PixelFormat) -> Self {
        Self {
            format,
            bytes_per_pixel: usize::from(format.bits_per_pixel / 8),
            colour_map: Vec::new(),
        }
    }

    /// The wire format.
    #[must_use]
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Bytes per pixel on the wire (1, 2 or 4).
    #[must_use]
    pub fn bytes_per_pixel(&self) -> usize {
        self.bytes_per_pixel
    }

    /// Updates colour-map entries (16-bit RGB) starting at `first`.
    pub fn set_colour_map(&mut self, first: u16, colours: &[[u16; 3]]) {
        let first = usize::from(first);
        let end = first + colours.len();
        if self.colour_map.len() < end {
            self.colour_map.resize(end, 0);
        }
        for (slot, [r, g, b]) in self.colour_map.iter_mut().skip(first).zip(colours) {
            *slot = (u32::from(r >> 8) << 16) | (u32::from(g >> 8) << 8) | u32::from(b >> 8);
        }
    }

    /// Reads one pixel value from the start of `bytes` (which must hold at
    /// least [`Self::bytes_per_pixel`] bytes).
    #[must_use]
    pub fn read_raw(&self, bytes: &[u8]) -> u32 {
        let mut value = 0u32;
        let pixel = bytes.get(..self.bytes_per_pixel).unwrap_or_default();
        if self.format.big_endian {
            for &b in pixel {
                value = (value << 8) | u32::from(b);
            }
        } else {
            for &b in pixel.iter().rev() {
                value = (value << 8) | u32::from(b);
            }
        }
        value
    }

    /// Converts a raw pixel value to `0x00RRGGBB`.
    #[must_use]
    pub fn to_rgb(&self, raw: u32) -> u32 {
        if !self.format.true_colour {
            return usize::try_from(raw)
                .ok()
                .and_then(|i| self.colour_map.get(i).copied())
                .unwrap_or(0);
        }
        let f = &self.format;
        let channel = |max: u16, shift: u8| -> u32 {
            let max = u32::from(max);
            let value = raw.checked_shr(u32::from(shift)).unwrap_or(0) & max;
            (value * 255 + max / 2) / max.max(1)
        };
        (channel(f.red_max, f.red_shift) << 16)
            | (channel(f.green_max, f.green_shift) << 8)
            | channel(f.blue_max, f.blue_shift)
    }

    /// Reads and converts one pixel.
    #[must_use]
    pub fn decode(&self, bytes: &[u8]) -> u32 {
        self.to_rgb(self.read_raw(bytes))
    }

    /// Converts `0x00RRGGBB` to the wire format (true-colour formats only)
    /// and appends it to `out`.
    pub fn encode(&self, rgb: u32, out: &mut Vec<u8>) {
        self.write_raw(self.to_raw(rgb), out);
    }

    /// Converts `0x00RRGGBB` to a raw pixel value in the wire format
    /// (true-colour formats only), before byte ordering.
    #[must_use]
    pub fn to_raw(&self, rgb: u32) -> u32 {
        let f = &self.format;
        let channel = |value: u32, max: u16, shift: u8| -> u32 {
            let max = u32::from(max);
            ((value * max + 127) / 255)
                .checked_shl(u32::from(shift))
                .unwrap_or(0)
        };
        channel((rgb >> 16) & 0xff, f.red_max, f.red_shift)
            | channel((rgb >> 8) & 0xff, f.green_max, f.green_shift)
            | channel(rgb & 0xff, f.blue_max, f.blue_shift)
    }

    /// Appends a raw pixel value as [`Self::bytes_per_pixel`] bytes in the
    /// format's byte order.
    pub fn write_raw(&self, raw: u32, out: &mut Vec<u8>) {
        let bytes = raw.to_le_bytes();
        let pixel = bytes.get(..self.bytes_per_pixel).unwrap_or_default();
        if self.format.big_endian {
            out.extend(pixel.iter().rev());
        } else {
            out.extend_from_slice(pixel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RGB565_BE: PixelFormat = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: true,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };

    #[test]
    fn rgb888_little_endian_is_identity() {
        let c = PixelConverter::new(PixelFormat::RGB888);
        assert_eq!(c.decode(&[0x33, 0x22, 0x11, 0x00]), 0x0011_2233);
        let mut out = Vec::new();
        c.encode(0x0011_2233, &mut out);
        assert_eq!(out, [0x33, 0x22, 0x11, 0x00]);
    }

    #[test]
    fn rgb565_scales_channels() {
        let c = PixelConverter::new(RGB565_BE);
        assert_eq!(c.decode(&[0xf8, 0x00]), 0x00ff_0000);
        assert_eq!(c.decode(&[0x07, 0xe0]), 0x0000_ff00);
        assert_eq!(c.decode(&[0x00, 0x1f]), 0x0000_00ff);
    }

    #[test]
    fn colour_map_lookup() {
        let mut c = PixelConverter::new(PixelFormat {
            true_colour: false,
            bits_per_pixel: 8,
            depth: 8,
            ..PixelFormat::RGB888
        });
        c.set_colour_map(2, &[[0xff00, 0x8000, 0x0000]]);
        assert_eq!(c.decode(&[2]), 0x00ff_8000);
        assert_eq!(c.decode(&[200]), 0);
    }

    proptest! {
        #[test]
        fn rgb888_round_trips(rgb in 0u32..0x0100_0000) {
            let c = PixelConverter::new(PixelFormat::RGB888);
            let mut out = Vec::new();
            c.encode(rgb, &mut out);
            prop_assert_eq!(c.decode(&out), rgb);
        }

        #[test]
        fn rgb565_round_trip_is_stable(rgb in 0u32..0x0100_0000) {
            let c = PixelConverter::new(RGB565_BE);
            let mut once = Vec::new();
            c.encode(rgb, &mut once);
            let mut twice = Vec::new();
            c.encode(c.decode(&once), &mut twice);
            prop_assert_eq!(once, twice);
        }
    }
}
