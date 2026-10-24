//! Hextile (encoding 5): 16×16 tiles, each raw or a background plus
//! subrectangles (rfbproto "Hextile Encoding").
//!
//! Hextile has no length prefix, so the payload is first measured with a
//! pure pass over the tile headers. Only when every byte is present is a
//! second pass made that paints the framebuffer.

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Rect};

use crate::util::{mul, need};
use crate::{Framebuffer, PixelConverter};

const TILE: u16 = 16;

const RAW: u8 = 1;
const BACKGROUND_SPECIFIED: u8 = 2;
const FOREGROUND_SPECIFIED: u8 = 4;
const ANY_SUBRECTS: u8 = 8;
const SUBRECTS_COLOURED: u8 = 16;
const KNOWN_BITS: u8 = 0x1f;

/// Splits `rect` into `size` × `size` tiles, left to right, top to bottom.
/// `rect` must already be known to fit the framebuffer.
pub(crate) fn tiles(rect: Rect, size: u16) -> impl Iterator<Item = Rect> {
    let step = usize::from(size.max(1));
    (0..rect.height).step_by(step).flat_map(move |ty| {
        (0..rect.width).step_by(step).map(move |tx| {
            Rect::new(
                rect.x.saturating_add(tx),
                rect.y.saturating_add(ty),
                size.min(rect.width - tx),
                size.min(rect.height - ty),
            )
        })
    })
}

/// Decodes a Hextile payload into `fb`.
pub(crate) fn decode(
    fb: &mut Framebuffer,
    converter: &PixelConverter,
    rect: Rect,
    input: &[u8],
) -> Result<Option<usize>, DecodeError> {
    fb.check(rect)?;
    let Some(len) = walk(converter, rect, input, None)? else {
        return Ok(None);
    };
    walk(converter, rect, input, Some(fb))?;
    Ok(Some(len))
}

/// Parses (and validates) every tile. Paints only when `fb` is given, so
/// with `None` it is a pure measurement.
fn walk(
    converter: &PixelConverter,
    rect: Rect,
    input: &[u8],
    mut fb: Option<&mut Framebuffer>,
) -> Result<Option<usize>, DecodeError> {
    let bpp = converter.bytes_per_pixel();
    let mut r = Reader::new(input);
    let mut background: Option<u32> = None;
    let mut foreground: Option<u32> = None;
    let mut scratch = [0u32; 256];
    for tile in tiles(rect, TILE) {
        let (tw, th) = (usize::from(tile.width), usize::from(tile.height));
        let mask = need!(r.u8());
        if mask & !KNOWN_BITS != 0 {
            return Err(DecodeError::Malformed("unknown Hextile subencoding bits"));
        }
        if mask & RAW != 0 {
            let pixels = need!(r.bytes(mul(tile.area(), bpp)?));
            if let Some(fb) = fb.as_deref_mut() {
                fb.write(tile, |i| {
                    converter.decode(pixels.get(i * bpp..).unwrap_or_default())
                })?;
            }
            background = None;
            foreground = None;
            continue;
        }
        if mask & BACKGROUND_SPECIFIED != 0 {
            background = Some(converter.decode(need!(r.bytes(bpp))));
        }
        let fill = background.ok_or(DecodeError::Malformed("Hextile tile has no background"))?;
        let coloured = mask & SUBRECTS_COLOURED != 0;
        if mask & FOREGROUND_SPECIFIED != 0 {
            if coloured {
                return Err(DecodeError::Malformed(
                    "Hextile foreground with coloured subrects",
                ));
            }
            foreground = Some(converter.decode(need!(r.bytes(bpp))));
        }
        let count = if mask & ANY_SUBRECTS == 0 {
            0
        } else {
            need!(r.u8())
        };
        let pixels = scratch.get_mut(..tw * th).unwrap_or_default();
        if fb.is_some() {
            pixels.fill(fill);
        }
        for _ in 0..count {
            let colour = if coloured {
                converter.decode(need!(r.bytes(bpp)))
            } else {
                foreground.ok_or(DecodeError::Malformed("Hextile subrect has no foreground"))?
            };
            let [xy, wh] = need!(r.array::<2>());
            let (sub_x, sub_y) = (usize::from(xy >> 4), usize::from(xy & 0x0f));
            let (sub_w, sub_h) = (usize::from(wh >> 4) + 1, usize::from(wh & 0x0f) + 1);
            if sub_x + sub_w > tw || sub_y + sub_h > th {
                return Err(DecodeError::Malformed("Hextile subrect outside its tile"));
            }
            if fb.is_some() {
                for row in pixels.chunks_exact_mut(tw).skip(sub_y).take(sub_h) {
                    row.get_mut(sub_x..sub_x + sub_w)
                        .unwrap_or_default()
                        .fill(colour);
                }
            }
        }
        if coloured {
            foreground = None;
        }
        if let Some(fb) = fb.as_deref_mut() {
            fb.write(tile, |i| pixels.get(i).copied().unwrap_or(0))?;
        }
    }
    Ok(Some(r.consumed()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use farsight_rfb::PixelFormat;
    use proptest::prelude::*;

    const RED: u32 = 0x00ff_0000;
    const BLUE: u32 = 0x0000_00ff;
    const GREEN: u32 = 0x0000_ff00;

    fn px(rgb: u32) -> [u8; 4] {
        rgb.to_le_bytes()
    }

    fn run(fb: &mut Framebuffer, rect: Rect, input: &[u8]) -> Result<Option<usize>, DecodeError> {
        decode(fb, &PixelConverter::new(PixelFormat::RGB888), rect, input)
    }

    #[test]
    fn tiles_cover_the_rectangle_with_short_edges() {
        let all: Vec<Rect> = tiles(Rect::new(1, 2, 20, 17), 16).collect();
        assert_eq!(
            all,
            [
                Rect::new(1, 2, 16, 16),
                Rect::new(17, 2, 4, 16),
                Rect::new(1, 18, 16, 1),
                Rect::new(17, 18, 4, 1),
            ]
        );
        assert_eq!(tiles(Rect::new(0, 0, 0, 5), 16).count(), 0);
    }

    /// Two tiles (16×2 and 2×2): the first has a red background and a blue
    /// 2×1 subrect at (1,1); the second inherits both colours.
    fn two_tile_payload() -> Vec<u8> {
        let mut p = vec![BACKGROUND_SPECIFIED | FOREGROUND_SPECIFIED | ANY_SUBRECTS];
        p.extend(px(RED));
        p.extend(px(BLUE));
        p.extend([1, 0x11, 0x10]); // count, x=1 y=1, w=2 h=1
        p.extend([ANY_SUBRECTS, 1, 0x00, 0x00]); // inherited colours, 1×1 at (0,0)
        p
    }

    #[test]
    fn subrects_and_inherited_colours_paint_tiles() {
        let mut fb = Framebuffer::new(18, 2);
        let payload = two_tile_payload();
        assert_eq!(
            run(&mut fb, Rect::new(0, 0, 18, 2), &payload),
            Ok(Some(payload.len()))
        );
        assert_eq!(fb.get(0, 0), Some(RED));
        assert_eq!(fb.get(1, 1), Some(BLUE));
        assert_eq!(fb.get(2, 1), Some(BLUE));
        assert_eq!(fb.get(3, 1), Some(RED));
        assert_eq!(fb.get(16, 0), Some(BLUE));
        assert_eq!(fb.get(17, 1), Some(RED));
    }

    #[test]
    fn truncation_at_every_cut_waits_without_painting() {
        let payload = two_tile_payload();
        for cut in 0..payload.len() {
            let mut fb = Framebuffer::new(18, 2);
            assert_eq!(
                run(&mut fb, Rect::new(0, 0, 18, 2), &payload[..cut]),
                Ok(None)
            );
            assert_eq!(fb, Framebuffer::new(18, 2), "cut {cut}");
        }
    }

    #[test]
    fn raw_tiles_and_coloured_subrects() {
        let mut p = vec![RAW];
        for rgb in [RED, GREEN, BLUE, RED] {
            p.extend(px(rgb));
        }
        let mut fb = Framebuffer::new(2, 2);
        assert_eq!(run(&mut fb, Rect::new(0, 0, 2, 2), &p), Ok(Some(17)));
        assert_eq!(fb.pixels(), [RED, GREEN, BLUE, RED]);

        let mut p = vec![BACKGROUND_SPECIFIED | ANY_SUBRECTS | SUBRECTS_COLOURED];
        p.extend(px(BLUE));
        p.push(2);
        p.extend(px(GREEN));
        p.extend([0x00, 0x00]);
        p.extend(px(RED));
        p.extend([0x11, 0x00]);
        assert_eq!(run(&mut fb, Rect::new(0, 0, 2, 2), &p), Ok(Some(p.len())));
        assert_eq!(fb.pixels(), [GREEN, BLUE, BLUE, RED]);
    }

    #[test]
    fn colours_are_not_carried_over_raw_or_coloured_tiles() {
        // A raw tile followed by a tile relying on an earlier background.
        let mut p = vec![BACKGROUND_SPECIFIED];
        p.extend(px(RED));
        p.push(RAW);
        p.extend([0; 16 * 4]);
        p.push(0);
        let mut fb = Framebuffer::new(48, 1);
        assert!(matches!(
            run(&mut fb, Rect::new(0, 0, 48, 1), &p),
            Err(DecodeError::Malformed(_))
        ));
        // A coloured-subrect tile followed by one relying on its foreground.
        let mut p = vec![BACKGROUND_SPECIFIED | ANY_SUBRECTS | SUBRECTS_COLOURED];
        p.extend(px(RED));
        p.extend([1, 0, 0, 0, 0, 0, 0]);
        p.extend([ANY_SUBRECTS, 1, 0, 0]);
        assert!(matches!(
            run(&mut fb, Rect::new(0, 0, 32, 1), &p),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn invalid_tiles_are_rejected() {
        let mut fb = Framebuffer::new(16, 16);
        let rect = Rect::new(0, 0, 4, 4);
        // No background on the first tile.
        assert!(run(&mut fb, rect, &[0]).is_err());
        // Subrect leaving the 4×4 tile.
        let mut p = vec![BACKGROUND_SPECIFIED | FOREGROUND_SPECIFIED | ANY_SUBRECTS];
        p.extend(px(RED));
        p.extend(px(BLUE));
        p.extend([1, 0x30, 0x10]);
        assert!(matches!(
            run(&mut fb, rect, &p),
            Err(DecodeError::Malformed(_))
        ));
        // Undefined subencoding bits.
        assert!(run(&mut fb, rect, &[0x20]).is_err());
        // Foreground together with coloured subrects.
        let mut p = vec![BACKGROUND_SPECIFIED | FOREGROUND_SPECIFIED | SUBRECTS_COLOURED];
        p.extend(px(RED));
        p.extend(px(BLUE));
        assert!(run(&mut fb, rect, &p).is_err());
        assert_eq!(fb, Framebuffer::new(16, 16));
    }

    #[test]
    fn rect_outside_the_framebuffer_is_rejected_before_waiting() {
        let mut fb = Framebuffer::new(8, 8);
        assert_eq!(
            run(&mut fb, Rect::new(4, 4, 8, 8), &[]),
            Err(DecodeError::RectOutOfBounds)
        );
    }

    #[test]
    fn sixteen_bit_pixels_use_the_converter() {
        let format = PixelFormat {
            bits_per_pixel: 16,
            depth: 16,
            big_endian: true,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..PixelFormat::RGB888
        };
        let mut fb = Framebuffer::new(2, 1);
        let p = [BACKGROUND_SPECIFIED, 0xf8, 0x00];
        assert_eq!(
            decode(
                &mut fb,
                &PixelConverter::new(format),
                Rect::new(0, 0, 2, 1),
                &p
            ),
            Ok(Some(3))
        );
        assert_eq!(fb.pixels(), [RED, RED]);
    }

    proptest! {
        #[test]
        fn decode_never_panics(
            bytes in proptest::collection::vec(any::<u8>(), 0..512),
            w in 0u16..40, h in 0u16..40,
        ) {
            let mut fb = Framebuffer::new(40, 40);
            let before = fb.clone();
            let result = run(&mut fb, Rect::new(0, 0, w, h), &bytes);
            if let Ok(Some(used)) = result {
                prop_assert!(used <= bytes.len());
            } else if result == Ok(None) {
                prop_assert_eq!(fb, before);
            }
        }
    }
}
