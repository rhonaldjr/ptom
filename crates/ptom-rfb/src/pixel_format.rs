//! The `PIXEL_FORMAT` structure (RFC 6143 §7.4).

use crate::{DecodeError, wire};

/// Length in bytes of an encoded pixel format, including 3 bytes of padding.
pub const PIXEL_FORMAT_LEN: usize = 16;

/// How pixel values are laid out on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)] // Mirrors the two wire flags exactly.
pub struct PixelFormat {
    /// Bits used per pixel on the wire: 8, 16 or 32.
    pub bits_per_pixel: u8,
    /// Useful bits within `bits_per_pixel`.
    pub depth: u8,
    /// Multi-byte pixels are big-endian.
    pub big_endian: bool,
    /// Pixels encode RGB directly; when false they index a colour map.
    pub true_colour: bool,
    /// Maximum red value (2^n − 1).
    pub red_max: u16,
    /// Maximum green value (2^n − 1).
    pub green_max: u16,
    /// Maximum blue value (2^n − 1).
    pub blue_max: u16,
    /// Bit position of the red channel.
    pub red_shift: u8,
    /// Bit position of the green channel.
    pub green_shift: u8,
    /// Bit position of the blue channel.
    pub blue_shift: u8,
}

impl PixelFormat {
    /// 32-bit little-endian true colour, 8 bits per channel: ptom's native format.
    pub const RGB888: Self = Self {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 16,
        green_shift: 8,
        blue_shift: 0,
    };

    /// Encodes the 16-byte wire representation.
    #[must_use]
    pub fn encode(&self) -> [u8; PIXEL_FORMAT_LEN] {
        let [red_hi, red_lo] = self.red_max.to_be_bytes();
        let [green_hi, green_lo] = self.green_max.to_be_bytes();
        let [blue_hi, blue_lo] = self.blue_max.to_be_bytes();
        [
            self.bits_per_pixel,
            self.depth,
            u8::from(self.big_endian),
            u8::from(self.true_colour),
            red_hi,
            red_lo,
            green_hi,
            green_lo,
            blue_hi,
            blue_lo,
            self.red_shift,
            self.green_shift,
            self.blue_shift,
            0,
            0,
            0,
        ]
    }

    /// Decodes and validates a pixel format received from a peer.
    ///
    /// # Errors
    /// Returns [`DecodeError::Truncated`] for short input and
    /// [`DecodeError::InvalidPixelFormat`] if the format is not one a
    /// conforming peer may send.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let &[
            bits_per_pixel,
            depth,
            big_endian,
            true_colour,
            red_hi,
            red_lo,
            green_hi,
            green_lo,
            blue_hi,
            blue_lo,
            red_shift,
            green_shift,
            blue_shift,
            _,
            _,
            _,
        ] = wire::take::<PIXEL_FORMAT_LEN>(bytes)?;
        let format = Self {
            bits_per_pixel,
            depth,
            big_endian: big_endian != 0,
            true_colour: true_colour != 0,
            red_max: u16::from_be_bytes([red_hi, red_lo]),
            green_max: u16::from_be_bytes([green_hi, green_lo]),
            blue_max: u16::from_be_bytes([blue_hi, blue_lo]),
            red_shift,
            green_shift,
            blue_shift,
        };
        format.validate()?;
        Ok(format)
    }

    /// Checks the constraints of RFC 6143 §7.4.
    ///
    /// # Errors
    /// Returns [`DecodeError::InvalidPixelFormat`] describing the first violation.
    pub fn validate(&self) -> Result<(), DecodeError> {
        if !matches!(self.bits_per_pixel, 8 | 16 | 32) {
            return Err(DecodeError::InvalidPixelFormat(
                "bits-per-pixel must be 8, 16 or 32",
            ));
        }
        if self.depth == 0 || self.depth > self.bits_per_pixel {
            return Err(DecodeError::InvalidPixelFormat(
                "depth must be between 1 and bits-per-pixel",
            ));
        }
        if self.true_colour {
            for (max, shift) in [
                (self.red_max, self.red_shift),
                (self.green_max, self.green_shift),
                (self.blue_max, self.blue_shift),
            ] {
                validate_channel(max, shift, self.bits_per_pixel)?;
            }
        }
        Ok(())
    }
}

fn validate_channel(max: u16, shift: u8, bits_per_pixel: u8) -> Result<(), DecodeError> {
    let max = u32::from(max);
    if max == 0 || max & (max + 1) != 0 {
        return Err(DecodeError::InvalidPixelFormat(
            "colour max must be 2^n - 1 and non-zero",
        ));
    }
    let channel_bits = u32::BITS - max.leading_zeros();
    if u32::from(shift) + channel_bits > u32::from(bits_per_pixel) {
        return Err(DecodeError::InvalidPixelFormat(
            "colour channel does not fit in bits-per-pixel",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const RGB565: PixelFormat = PixelFormat {
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
    fn rgb888_wire_layout_matches_rfc() {
        assert_eq!(
            PixelFormat::RGB888.encode(),
            [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]
        );
    }

    #[test]
    fn known_formats_round_trip() {
        for format in [PixelFormat::RGB888, RGB565] {
            assert_eq!(PixelFormat::decode(&format.encode()), Ok(format));
        }
    }

    #[test]
    fn colour_map_formats_skip_channel_checks() {
        let indexed = PixelFormat {
            bits_per_pixel: 8,
            depth: 8,
            big_endian: false,
            true_colour: false,
            red_max: 0,
            green_max: 0,
            blue_max: 0,
            red_shift: 0,
            green_shift: 0,
            blue_shift: 0,
        };
        assert_eq!(indexed.validate(), Ok(()));
    }

    #[test]
    fn rejects_invalid_formats() {
        let cases = [
            PixelFormat {
                bits_per_pixel: 24,
                ..PixelFormat::RGB888
            },
            PixelFormat {
                depth: 0,
                ..PixelFormat::RGB888
            },
            PixelFormat {
                depth: 33,
                ..PixelFormat::RGB888
            },
            PixelFormat {
                red_max: 0,
                ..PixelFormat::RGB888
            },
            PixelFormat {
                red_max: 200,
                ..PixelFormat::RGB888
            },
            PixelFormat {
                red_shift: 25,
                ..PixelFormat::RGB888
            },
        ];
        for format in cases {
            assert!(
                matches!(format.validate(), Err(DecodeError::InvalidPixelFormat(_))),
                "{format:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_truncated_input() {
        assert_eq!(
            PixelFormat::decode(&[32, 24, 0]),
            Err(DecodeError::Truncated { needed: 16, got: 3 })
        );
    }

    proptest! {
        #[test]
        fn decode_never_panics_and_accepted_formats_round_trip(
            bytes in proptest::collection::vec(any::<u8>(), 0..24)
        ) {
            if let Ok(format) = PixelFormat::decode(&bytes) {
                prop_assert_eq!(PixelFormat::decode(&format.encode()), Ok(format));
            }
        }
    }
}
