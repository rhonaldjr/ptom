//! Server-side encoders: Raw, CopyRect, RRE, Hextile, ZRLE ([`ZrleEncoder`]),
//! Tight with JPEG ([`TightEncoder`]), and the Cursor, `CursorWithAlpha`,
//! `ExtendedDesktopSize` and `DesktopName` pseudo-encodings. Every encoder's output decodes with
//! [`crate::Decoder`] to the same pixels (property-tested in both directions).

mod hextile;
mod palette;
mod rre;
mod subrects;
mod tight;
mod zrle;

use farsight_rfb::wire::WriteExt;
use farsight_rfb::{DecodeError, Encoding, Rect, Screen};

pub use hextile::hextile;
pub use rre::rre;
pub use tight::{TIGHT_MAX_AREA, TIGHT_MAX_WIDTH, TightEncoder, jpeg_capable};
pub use zrle::ZrleEncoder;

/// Longest desktop name sent or accepted (`DesktopName` pseudo-encoding).
pub const MAX_DESKTOP_NAME: usize = 1024;

/// Encodes a `DesktopName` pseudo-rectangle payload: length then UTF-8,
/// cut at [`MAX_DESKTOP_NAME`] bytes on a character boundary.
#[must_use]
pub fn desktop_name(name: &str) -> Vec<u8> {
    let mut end = name.len().min(MAX_DESKTOP_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let bytes = name.as_bytes().get(..end).unwrap_or_default();
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.put_u32(u32::try_from(bytes.len()).unwrap_or(0));
    out.extend_from_slice(bytes);
    out
}

use crate::{Cursor, Framebuffer, PixelConverter};

/// Encodes `rect` of `framebuffer` as a Raw payload in the client's format.
///
/// # Errors
/// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the framebuffer.
pub fn raw(
    framebuffer: &Framebuffer,
    rect: Rect,
    converter: &PixelConverter,
) -> Result<Vec<u8>, DecodeError> {
    let pixels = framebuffer.read(rect)?;
    let mut out = Vec::with_capacity(pixels.len() * converter.bytes_per_pixel());
    for rgb in pixels {
        converter.encode(rgb, &mut out);
    }
    Ok(out)
}

/// Encodes a CopyRect payload: the source position.
#[must_use]
pub fn copy_rect(src_x: u16, src_y: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    out.put_u16(src_x);
    out.put_u16(src_y);
    out
}

/// Encodes a Cursor pseudo-rectangle payload: the shape in the client's
/// pixel format, then a 1-bit mask (pixels with alpha of at least 128 are
/// opaque). The rectangle header carries the hotspot and size.
#[must_use]
pub fn cursor(cursor: &Cursor, converter: &PixelConverter) -> Vec<u8> {
    let width = usize::from(cursor.width());
    let row_bytes = width.div_ceil(8);
    let pixels = cursor.rgba().as_chunks::<4>().0;
    let mut out = Vec::with_capacity(pixels.len() * converter.bytes_per_pixel() + row_bytes);
    for &[r, g, b, _] in pixels {
        converter.encode(
            (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
            &mut out,
        );
    }
    if width == 0 {
        return out;
    }
    for row in pixels.chunks(width) {
        for group in row.chunks(8) {
            let bits = group
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &[_, _, _, a])| {
                    acc | (u8::from(a >= 128) << (7 - i))
                });
            out.push(bits);
        }
    }
    out
}

/// Encodes a `CursorWithAlpha` pseudo-rectangle payload: the Raw encoding
/// number, then pre-multiplied RGBA.
#[must_use]
pub fn cursor_with_alpha(cursor: &Cursor) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + cursor.rgba().len());
    out.put_i32(Encoding::Raw.code());
    for &[r, g, b, a] in cursor.rgba().as_chunks::<4>().0 {
        out.extend([premultiply(r, a), premultiply(g, a), premultiply(b, a), a]);
    }
    out
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    let value = (u32::from(channel) * u32::from(alpha) + 127) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// Encodes an `ExtendedDesktopSize` payload: the screen count (at most 255
/// screens are sent), padding and the screens. The rectangle header carries
/// the reason (x), status (y) and framebuffer size.
#[must_use]
pub fn extended_desktop_size(screens: &[Screen]) -> Vec<u8> {
    let screens = screens.get(..255).unwrap_or(screens);
    let mut out = Vec::with_capacity(4 + screens.len() * farsight_rfb::screen::SCREEN_LEN);
    out.put_u8(u8::try_from(screens.len()).unwrap_or(u8::MAX));
    out.put_padding(3);
    for screen in screens {
        screen.encode(&mut out);
    }
    out
}
