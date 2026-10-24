//! RRE (encoding 2): a background plus subrectangles (rfbproto "RRE Encoding").

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Rect};

use crate::util::{mul, need};
use crate::{Framebuffer, PixelConverter};

/// Most times each pixel may be painted by subrects (overlap allowance).
const MAX_OVERDRAW: usize = 2;

/// Decodes an RRE payload into `fb`. The subrectangle count is checked
/// against the bytes available and the rectangle's area before any work.
pub(crate) fn decode(
    fb: &mut Framebuffer,
    converter: &PixelConverter,
    rect: Rect,
    input: &[u8],
) -> Result<Option<usize>, DecodeError> {
    fb.check(rect)?;
    let bpp = converter.bytes_per_pixel();
    let mut reader = Reader::new(input);
    let count = usize::try_from(need!(reader.u32())).unwrap_or(usize::MAX);
    if count > rect.area() {
        return Err(DecodeError::Malformed("more RRE subrects than pixels"));
    }
    let total = 4 + bpp + mul(count, bpp + 8)?;
    if input.len() < total {
        return Ok(None);
    }
    let background = converter.decode(need!(reader.bytes(bpp)));
    let mut subrects = Vec::with_capacity(count);
    // Honest encoders don't overlap subrects; many full-size ones would make
    // a few bytes cost millions of pixel writes.
    let mut painted = 0usize;
    for _ in 0..count {
        let colour = converter.decode(need!(reader.bytes(bpp)));
        let (sub_x, sub_y, sub_w, sub_h) = (
            need!(reader.u16()),
            need!(reader.u16()),
            need!(reader.u16()),
            need!(reader.u16()),
        );
        if u32::from(sub_x) + u32::from(sub_w) > u32::from(rect.width)
            || u32::from(sub_y) + u32::from(sub_h) > u32::from(rect.height)
        {
            return Err(DecodeError::Malformed("RRE subrect outside its rectangle"));
        }
        painted = painted.saturating_add(usize::from(sub_w) * usize::from(sub_h));
        if painted > rect.area().saturating_mul(MAX_OVERDRAW) {
            return Err(DecodeError::Malformed("RRE subrects overlap too much"));
        }
        subrects.push((
            colour,
            Rect::new(rect.x + sub_x, rect.y + sub_y, sub_w, sub_h),
        ));
    }
    fb.fill(rect, background)?;
    for (colour, sub) in subrects {
        fb.fill(sub, colour)?;
    }
    Ok(Some(reader.consumed()))
}
