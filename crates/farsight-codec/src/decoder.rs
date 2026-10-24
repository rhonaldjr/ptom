//! The viewer-side rectangle decoder.

use farsight_rfb::client::RectDecoder;
use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Encoding, Limits, PixelFormat, RectHeader};

use crate::{Framebuffer, PixelConverter};

/// Decodes rectangles into a [`Framebuffer`].
#[derive(Debug, Clone)]
pub struct Decoder {
    framebuffer: Framebuffer,
    converter: PixelConverter,
    limits: Limits,
}

impl Decoder {
    /// Creates a decoder for a `width` × `height` desktop sent in `format`.
    #[must_use]
    pub fn new(width: u16, height: u16, format: PixelFormat, limits: Limits) -> Self {
        Self {
            framebuffer: Framebuffer::new(width, height),
            converter: PixelConverter::new(format),
            limits,
        }
    }

    /// The decoded screen.
    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }

    /// Switches to the pixel format just requested with `SetPixelFormat`.
    pub fn set_pixel_format(&mut self, format: PixelFormat) {
        self.converter = PixelConverter::new(format);
    }

    /// Applies colour-map entries sent by the server.
    pub fn set_colour_map(&mut self, first: u16, colours: &[[u16; 3]]) {
        self.converter.set_colour_map(first, colours);
    }

    fn resize(&mut self, width: u16, height: u16) -> Result<Option<usize>, DecodeError> {
        let max = self.limits.max_framebuffer_side;
        if width > max || height > max {
            return Err(DecodeError::LimitExceeded {
                what: "framebuffer side",
                len: u64::from(width.max(height)),
                max: u64::from(max),
            });
        }
        self.framebuffer.resize(width, height);
        Ok(Some(0))
    }

    fn raw(&mut self, header: &RectHeader, input: &[u8]) -> Result<Option<usize>, DecodeError> {
        let bpp = self.converter.bytes_per_pixel();
        let len = header.rect.area() * bpp;
        let Some(payload) = input.get(..len) else {
            return Ok(None);
        };
        let converter = &self.converter;
        self.framebuffer.write(header.rect, |i| {
            converter.decode(payload.get(i * bpp..).unwrap_or_default())
        })?;
        Ok(Some(len))
    }

    fn copy_rect(
        &mut self,
        header: &RectHeader,
        input: &[u8],
    ) -> Result<Option<usize>, DecodeError> {
        let mut r = Reader::new(input);
        let (Ok(src_x), Ok(src_y)) = (r.u16(), r.u16()) else {
            return Ok(None);
        };
        self.framebuffer.copy(src_x, src_y, header.rect)?;
        Ok(Some(4))
    }
}

impl RectDecoder for Decoder {
    fn decode_rect(
        &mut self,
        header: &RectHeader,
        input: &[u8],
    ) -> Result<Option<usize>, DecodeError> {
        match header.encoding {
            Encoding::DesktopSize => self.resize(header.rect.width, header.rect.height),
            Encoding::Raw => self.raw(header, input),
            Encoding::CopyRect => self.copy_rect(header, input),
            other => Err(DecodeError::UnsupportedEncoding(other.code())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode;
    use farsight_rfb::Rect;

    fn decoder() -> Decoder {
        Decoder::new(4, 4, PixelFormat::RGB888, Limits::DEFAULT)
    }

    #[test]
    fn raw_waits_for_the_whole_payload_without_side_effects() {
        let mut d = decoder();
        let header = RectHeader {
            rect: Rect::new(0, 0, 2, 1),
            encoding: Encoding::Raw,
        };
        let payload = [1, 0, 0, 0, 2, 0, 0, 0];
        assert_eq!(d.decode_rect(&header, &payload[..7]), Ok(None));
        assert_eq!(d.framebuffer().get(0, 0), Some(0));
        assert_eq!(d.decode_rect(&header, &payload), Ok(Some(8)));
        assert_eq!(d.framebuffer().read(Rect::new(0, 0, 2, 1)).unwrap(), [1, 2]);
    }

    #[test]
    fn raw_rejects_rects_outside_the_framebuffer() {
        let mut d = decoder();
        let header = RectHeader {
            rect: Rect::new(3, 3, 2, 2),
            encoding: Encoding::Raw,
        };
        assert_eq!(
            d.decode_rect(&header, &[0; 16]),
            Err(DecodeError::RectOutOfBounds)
        );
    }

    #[test]
    fn raw_encoder_and_decoder_agree() {
        let mut source = Framebuffer::new(4, 4);
        source
            .write(Rect::new(0, 0, 4, 4), |i| {
                u32::try_from(i * 0x0001_0101).unwrap()
            })
            .unwrap();
        let rect = Rect::new(1, 1, 3, 2);
        let payload =
            encode::raw(&source, rect, &PixelConverter::new(PixelFormat::RGB888)).unwrap();
        let mut d = decoder();
        let header = RectHeader {
            rect,
            encoding: Encoding::Raw,
        };
        assert_eq!(d.decode_rect(&header, &payload), Ok(Some(payload.len())));
        assert_eq!(d.framebuffer().read(rect), source.read(rect));
    }

    #[test]
    fn copy_rect_moves_pixels() {
        let mut d = decoder();
        d.decode_rect(
            &RectHeader {
                rect: Rect::new(0, 0, 1, 1),
                encoding: Encoding::Raw,
            },
            &[9, 0, 0, 0],
        )
        .unwrap();
        let header = RectHeader {
            rect: Rect::new(3, 3, 1, 1),
            encoding: Encoding::CopyRect,
        };
        assert_eq!(d.decode_rect(&header, &[0, 0]), Ok(None));
        assert_eq!(d.decode_rect(&header, &[0, 0, 0, 0]), Ok(Some(4)));
        assert_eq!(d.framebuffer().get(3, 3), Some(9));
    }

    #[test]
    fn desktop_size_resizes_within_limits() {
        let mut d = decoder();
        let header = RectHeader {
            rect: Rect::new(0, 0, 800, 600),
            encoding: Encoding::DesktopSize,
        };
        assert_eq!(d.decode_rect(&header, &[]), Ok(Some(0)));
        assert_eq!(
            (d.framebuffer().width(), d.framebuffer().height()),
            (800, 600)
        );
        let huge = RectHeader {
            rect: Rect::new(0, 0, 60000, 10),
            encoding: Encoding::DesktopSize,
        };
        assert!(d.decode_rect(&huge, &[]).is_err());
    }

    #[test]
    fn unsupported_encodings_are_errors() {
        let mut d = decoder();
        let header = RectHeader {
            rect: Rect::new(0, 0, 1, 1),
            encoding: Encoding::Unknown(99),
        };
        assert_eq!(
            d.decode_rect(&header, &[]),
            Err(DecodeError::UnsupportedEncoding(99))
        );
    }
}
