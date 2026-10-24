//! Hextile encoder (rfbproto "Hextile Encoding"): 16×16 tiles, each solid,
//! two-colour, coloured subrectangles or raw, whichever is smallest.

use farsight_rfb::{DecodeError, Rect};

use super::subrects::{self, Subrect};
use crate::hextile::tiles;
use crate::{Framebuffer, PixelConverter};

const TILE: u16 = 16;
const RAW: u8 = 1;
const BACKGROUND_SPECIFIED: u8 = 2;
const FOREGROUND_SPECIFIED: u8 = 4;
const ANY_SUBRECTS: u8 = 8;
const SUBRECTS_COLOURED: u8 = 16;
/// A tile has at most 255 subrectangles (count is one byte).
const MAX_SUBRECTS: usize = 255;

/// Encodes `rect` of `framebuffer` as Hextile in the client's format.
///
/// # Errors
/// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the framebuffer.
pub fn hextile(
    framebuffer: &Framebuffer,
    rect: Rect,
    converter: &PixelConverter,
) -> Result<Vec<u8>, DecodeError> {
    framebuffer.check(rect)?;
    let bpp = converter.bytes_per_pixel();
    let mut out = Vec::new();
    // What the decoder remembers between tiles (forgotten after raw or coloured tiles).
    let mut last_background: Option<u32> = None;
    let mut last_foreground: Option<u32> = None;
    for tile in tiles(rect, TILE) {
        let raw: Vec<u32> = framebuffer
            .read(tile)?
            .into_iter()
            .map(|rgb| converter.to_raw(rgb))
            .collect();
        let width = usize::from(tile.width);
        let background = subrects::background(&raw);
        let rects = subrects::find(&raw, width, background, MAX_SUBRECTS);
        let raw_size = 1 + raw.len() * bpp;
        let single_fg = rects.as_ref().and_then(|r| one_colour(r));
        let encoded_size = rects.as_ref().map(|r| {
            let header = 1 + if last_background == Some(background) {
                0
            } else {
                bpp
            };
            match single_fg {
                Some(fg) => {
                    header
                        + if last_foreground == Some(fg) { 0 } else { bpp }
                        + usize::from(!r.is_empty())
                        + r.len() * 2
                }
                None => header + 1 + r.len() * (bpp + 2),
            }
        });
        match (rects, encoded_size) {
            (Some(rects), Some(size)) if size < raw_size => {
                let mut mask = 0;
                if last_background != Some(background) {
                    mask |= BACKGROUND_SPECIFIED;
                }
                let coloured = !rects.is_empty() && single_fg.is_none();
                if let Some(fg) = single_fg
                    && last_foreground != Some(fg)
                    && !rects.is_empty()
                {
                    mask |= FOREGROUND_SPECIFIED;
                }
                if !rects.is_empty() {
                    mask |= ANY_SUBRECTS;
                }
                if coloured {
                    mask |= SUBRECTS_COLOURED;
                }
                out.push(mask);
                if mask & BACKGROUND_SPECIFIED != 0 {
                    converter.write_raw(background, &mut out);
                    last_background = Some(background);
                }
                if mask & FOREGROUND_SPECIFIED != 0
                    && let Some(fg) = single_fg
                {
                    converter.write_raw(fg, &mut out);
                    last_foreground = Some(fg);
                }
                if !rects.is_empty() {
                    out.push(u8::try_from(rects.len()).unwrap_or(u8::MAX));
                    for r in &rects {
                        if coloured {
                            converter.write_raw(r.colour, &mut out);
                        }
                        out.extend(packed(r));
                    }
                }
                if coloured {
                    last_foreground = None;
                }
            }
            _ => {
                out.push(RAW);
                for &pixel in &raw {
                    converter.write_raw(pixel, &mut out);
                }
                last_background = None;
                last_foreground = None;
            }
        }
    }
    Ok(out)
}

/// The colour all `rects` share, if they share one (and there is one).
fn one_colour(rects: &[Subrect]) -> Option<u32> {
    let first = rects.first()?.colour;
    rects.iter().all(|r| r.colour == first).then_some(first)
}

/// x/y and width-1/height-1, a nibble each (tiles are at most 16×16).
fn packed(r: &Subrect) -> [u8; 2] {
    let nibble = |v: usize| u8::try_from(v & 0x0f).unwrap_or(0);
    [
        nibble(r.x) << 4 | nibble(r.y),
        nibble(r.width - 1) << 4 | nibble(r.height - 1),
    ]
}
