//! RRE encoder (rfbproto "RRE Encoding"): a background colour and
//! single-colour subrectangles. Good for large flat areas.

use farsight_rfb::wire::WriteExt;
use farsight_rfb::{DecodeError, Rect};

use super::subrects;
use crate::{Framebuffer, PixelConverter};

/// Encodes `rect` as RRE, or returns `None` when more than `max_subrects`
/// rectangles would be needed (then another encoding is smaller).
///
/// # Errors
/// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the framebuffer.
pub fn rre(
    framebuffer: &Framebuffer,
    rect: Rect,
    converter: &PixelConverter,
    max_subrects: usize,
) -> Result<Option<Vec<u8>>, DecodeError> {
    let raw: Vec<u32> = framebuffer
        .read(rect)?
        .into_iter()
        .map(|rgb| converter.to_raw(rgb))
        .collect();
    if raw.is_empty() {
        return Ok(None);
    }
    let background = subrects::background(&raw);
    let Some(rects) = subrects::find(&raw, usize::from(rect.width), background, max_subrects)
    else {
        return Ok(None);
    };
    let bpp = converter.bytes_per_pixel();
    let mut out = Vec::with_capacity(4 + bpp + rects.len() * (bpp + 8));
    out.put_u32(u32::try_from(rects.len()).unwrap_or(u32::MAX));
    converter.write_raw(background, &mut out);
    for r in rects {
        converter.write_raw(r.colour, &mut out);
        // Within a u16-sized rect, so these fit.
        for v in [r.x, r.y, r.width, r.height] {
            out.put_u16(u16::try_from(v).unwrap_or(u16::MAX));
        }
    }
    Ok(Some(out))
}
