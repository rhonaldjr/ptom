//! Cursor shapes: the Cursor (-239) and Cursor With Alpha (-314)
//! pseudo-encodings (rfbproto "Cursor Pseudo-encoding",
//! "Cursor With Alpha Pseudo-encoding").

use std::fmt;

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Encoding, RectHeader};

use crate::PixelConverter;
use crate::util::{mul, need};

/// Largest cursor width or height accepted (larger shapes are rejected so a
/// hostile server cannot make the viewer allocate huge cursor images).
pub const MAX_CURSOR_SIDE: u16 = 256;

/// A cursor shape as straight (not pre-multiplied) RGBA, 4 bytes per pixel,
/// row-major. A 0×0 cursor means "hide the pointer".
#[derive(Clone, PartialEq, Eq)]
pub struct Cursor {
    width: u16,
    height: u16,
    hotspot: (u16, u16),
    rgba: Vec<u8>,
}

impl fmt::Debug for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cursor")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("hotspot", &self.hotspot)
            .finish_non_exhaustive()
    }
}

impl Cursor {
    fn new(width: u16, height: u16, hotspot: (u16, u16), rgba: Vec<u8>) -> Self {
        // Servers occasionally send a hotspot outside the shape; clamp it.
        let hotspot = (
            hotspot.0.min(width.saturating_sub(1)),
            hotspot.1.min(height.saturating_sub(1)),
        );
        Self {
            width,
            height,
            hotspot,
            rgba,
        }
    }

    /// Creates a cursor from straight-alpha RGBA bytes (`width * height * 4`
    /// of them), as captured on the server. Returns `None` if the length is
    /// wrong or a side exceeds [`MAX_CURSOR_SIDE`]. The hotspot is clamped
    /// into the shape.
    #[must_use]
    pub fn from_rgba(width: u16, height: u16, hotspot: (u16, u16), rgba: Vec<u8>) -> Option<Self> {
        let expected = usize::from(width)
            .checked_mul(usize::from(height))?
            .checked_mul(4)?;
        (width <= MAX_CURSOR_SIDE && height <= MAX_CURSOR_SIDE && rgba.len() == expected)
            .then(|| Self::new(width, height, hotspot, rgba))
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

    /// The hotspot (the pixel that is the pointer position), inside the shape.
    #[must_use]
    pub fn hotspot(&self) -> (u16, u16) {
        self.hotspot
    }

    /// Straight-alpha RGBA bytes, `width * height * 4` of them.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Whether the server asked for no visible cursor.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

fn check_size(header: &RectHeader) -> Result<(), DecodeError> {
    let side = header.rect.width.max(header.rect.height);
    if side > MAX_CURSOR_SIDE {
        return Err(DecodeError::LimitExceeded {
            what: "cursor side",
            len: u64::from(side),
            max: u64::from(MAX_CURSOR_SIDE),
        });
    }
    Ok(())
}

/// Decodes a Cursor pseudo-rectangle: pixels in the session's pixel format,
/// then a 1-bit-per-pixel validity mask with rows padded to whole bytes.
pub(crate) fn decode_cursor(
    converter: &PixelConverter,
    header: &RectHeader,
    input: &[u8],
) -> Result<Option<(Cursor, usize)>, DecodeError> {
    check_size(header)?;
    let (width, height) = (header.rect.width, header.rect.height);
    let bpp = converter.bytes_per_pixel();
    let row_bytes = usize::from(width).div_ceil(8);
    let mut r = Reader::new(input);
    let pixels = need!(r.bytes(mul(header.rect.area(), bpp)?));
    let mask = need!(r.bytes(mul(row_bytes, usize::from(height))?));
    let mut rgba = Vec::with_capacity(header.rect.area() * 4);
    let stride = usize::from(width).max(1);
    for (i, pixel) in pixels.chunks_exact(bpp.max(1)).enumerate() {
        let (y, x) = (i / stride, i % stride);
        let bits = mask.get(y * row_bytes + x / 8).copied().unwrap_or(0);
        if (bits >> (7 - x % 8)) & 1 == 1 {
            let [_, red, green, blue] = converter.decode(pixel).to_be_bytes();
            rgba.extend([red, green, blue, 255]);
        } else {
            rgba.extend([0, 0, 0, 0]);
        }
    }
    let cursor = Cursor::new(width, height, (header.rect.x, header.rect.y), rgba);
    Ok(Some((cursor, r.consumed())))
}

/// Decodes a Cursor With Alpha pseudo-rectangle. Only the Raw sub-encoding
/// is supported: R, G, B, A bytes per pixel with pre-multiplied alpha.
pub(crate) fn decode_alpha_cursor(
    header: &RectHeader,
    input: &[u8],
) -> Result<Option<(Cursor, usize)>, DecodeError> {
    check_size(header)?;
    let mut r = Reader::new(input);
    let encoding = need!(r.i32());
    if Encoding::from_code(encoding) != Encoding::Raw {
        return Err(DecodeError::UnsupportedEncoding(encoding));
    }
    let pixels = need!(r.bytes(mul(header.rect.area(), 4)?));
    let mut rgba = Vec::with_capacity(pixels.len());
    for &[red, green, blue, alpha] in pixels.as_chunks::<4>().0 {
        rgba.extend([
            unpremultiply(red, alpha),
            unpremultiply(green, alpha),
            unpremultiply(blue, alpha),
            alpha,
        ]);
    }
    let cursor = Cursor::new(
        header.rect.width,
        header.rect.height,
        (header.rect.x, header.rect.y),
        rgba,
    );
    Ok(Some((cursor, r.consumed())))
}

fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    let (c, a) = (u32::from(channel), u32::from(alpha));
    u8::try_from(((c * 255 + a / 2) / a).min(255)).unwrap_or(u8::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use farsight_rfb::{PixelFormat, Rect};
    use proptest::prelude::*;

    fn header(x: u16, y: u16, w: u16, h: u16, encoding: Encoding) -> RectHeader {
        RectHeader {
            rect: Rect::new(x, y, w, h),
            encoding,
        }
    }

    #[test]
    fn cursor_uses_the_mask_for_alpha() {
        let conv = PixelConverter::new(PixelFormat::RGB888);
        // 9×2 so mask rows are 2 bytes; pixel (x, y) has value x + 16 y.
        let mut payload = Vec::new();
        for y in 0..2u32 {
            for x in 0..9u32 {
                payload.extend((0x0010_2000 + x + 16 * y).to_le_bytes());
            }
        }
        payload.extend([0b1000_0000, 0b1000_0000, 0b0100_0000, 0]);
        let h = header(3, 1, 9, 2, Encoding::Cursor);
        let (cursor, used) = decode_cursor(&conv, &h, &payload).unwrap().unwrap();
        assert_eq!(used, payload.len());
        assert_eq!(
            (cursor.width(), cursor.height(), cursor.hotspot()),
            (9, 2, (3, 1))
        );
        let px = |x: usize, y: usize| &cursor.rgba()[(y * 9 + x) * 4..][..4];
        assert_eq!(px(0, 0), [0x10, 0x20, 0x00, 255]);
        assert_eq!(px(1, 0), [0, 0, 0, 0]);
        assert_eq!(px(8, 0), [0x10, 0x20, 0x08, 255]);
        assert_eq!(px(1, 1), [0x10, 0x20, 0x11, 255]);
        assert_eq!(px(0, 1), [0, 0, 0, 0]);
        for cut in 0..payload.len() {
            assert_eq!(decode_cursor(&conv, &h, &payload[..cut]), Ok(None));
        }
    }

    #[test]
    fn empty_cursor_hides_the_pointer() {
        let conv = PixelConverter::new(PixelFormat::RGB888);
        let (cursor, used) = decode_cursor(&conv, &header(0, 0, 0, 0, Encoding::Cursor), &[])
            .unwrap()
            .unwrap();
        assert_eq!(used, 0);
        assert!(cursor.is_empty());
        assert_eq!(cursor.hotspot(), (0, 0));
    }

    #[test]
    fn hotspot_is_clamped_into_the_shape() {
        let conv = PixelConverter::new(PixelFormat::RGB888);
        let (cursor, _) = decode_cursor(
            &conv,
            &header(50, 60, 1, 1, Encoding::Cursor),
            &[0, 0, 0, 0, 0x80],
        )
        .unwrap()
        .unwrap();
        assert_eq!(cursor.hotspot(), (0, 0));
    }

    #[test]
    fn oversized_cursors_are_rejected_before_waiting() {
        let conv = PixelConverter::new(PixelFormat::RGB888);
        assert!(matches!(
            decode_cursor(&conv, &header(0, 0, 257, 1, Encoding::Cursor), &[]),
            Err(DecodeError::LimitExceeded {
                what: "cursor side",
                ..
            })
        ));
        assert!(matches!(
            decode_alpha_cursor(&header(0, 0, 1, 60000, Encoding::CursorWithAlpha), &[]),
            Err(DecodeError::LimitExceeded { .. })
        ));
        assert!(decode_cursor(&conv, &header(0, 0, 256, 256, Encoding::Cursor), &[]).is_ok());
    }

    #[test]
    fn alpha_cursor_is_unpremultiplied() {
        let h = header(1, 0, 2, 1, Encoding::CursorWithAlpha);
        let payload = [0, 0, 0, 0, 0x80, 0x40, 0x00, 0x80, 0xff, 0x00, 0x10, 0xff];
        let (cursor, used) = decode_alpha_cursor(&h, &payload).unwrap().unwrap();
        assert_eq!(used, 12);
        assert_eq!(
            cursor.rgba(),
            [0xff, 0x80, 0x00, 0x80, 0xff, 0x00, 0x10, 0xff]
        );
        assert_eq!(cursor.hotspot(), (1, 0));
        for cut in 0..payload.len() {
            assert_eq!(decode_alpha_cursor(&h, &payload[..cut]), Ok(None));
        }
    }

    #[test]
    fn alpha_cursor_supports_only_raw() {
        let h = header(0, 0, 1, 1, Encoding::CursorWithAlpha);
        assert_eq!(
            decode_alpha_cursor(&h, &[0, 0, 0, 16]),
            Err(DecodeError::UnsupportedEncoding(16))
        );
    }

    #[test]
    fn unpremultiply_handles_edges() {
        assert_eq!(unpremultiply(0, 0), 0);
        assert_eq!(unpremultiply(200, 0), 0);
        assert_eq!(unpremultiply(255, 255), 255);
        assert_eq!(unpremultiply(200, 100), 255);
        assert_eq!(unpremultiply(50, 100), 128);
    }

    proptest! {
        #[test]
        fn decode_never_panics(
            bytes in proptest::collection::vec(any::<u8>(), 0..512),
            x in any::<u16>(), y in any::<u16>(), w in 0u16..300, h in 0u16..20,
        ) {
            let conv = PixelConverter::new(PixelFormat::RGB888);
            if let Ok(Some((cursor, used))) =
                decode_cursor(&conv, &header(x, y, w, h, Encoding::Cursor), &bytes)
            {
                prop_assert!(used <= bytes.len());
                prop_assert_eq!(cursor.rgba().len(), usize::from(w) * usize::from(h) * 4);
            }
            if let Ok(Some((cursor, used))) =
                decode_alpha_cursor(&header(x, y, w, h, Encoding::CursorWithAlpha), &bytes)
            {
                prop_assert!(used <= bytes.len());
                prop_assert_eq!(cursor.rgba().len(), usize::from(w) * usize::from(h) * 4);
            }
        }
    }
}
