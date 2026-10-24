//! Server-side encoders.

use farsight_rfb::wire::WriteExt;
use farsight_rfb::{DecodeError, Rect};

use crate::{Framebuffer, PixelConverter};

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
