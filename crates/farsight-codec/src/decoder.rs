//! The viewer-side rectangle decoder.

use farsight_rfb::client::RectDecoder;
use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Encoding, Limits, PixelFormat, RectHeader, Screen};

use crate::cursor::{self, Cursor};
use crate::desktop::{self, DesktopResize};
use crate::inflate::ZlibStream;
use crate::util::need;
use crate::{Framebuffer, PixelConverter, hextile, tight, zrle};

/// Decodes rectangles into a [`Framebuffer`], and keeps the per-connection
/// state that encodings share: the ZRLE and Tight zlib streams, the cursor
/// shape and the screen layout.
#[derive(Debug)]
pub struct Decoder {
    framebuffer: Framebuffer,
    converter: PixelConverter,
    limits: Limits,
    zrle: ZlibStream,
    tight: [ZlibStream; tight::STREAMS],
    cursor: Option<Cursor>,
    cursor_generation: u64,
    screens: Vec<Screen>,
    extended_resize: bool,
    last_resize: Option<DesktopResize>,
    /// A new desktop name, until taken with [`Self::take_name`].
    name: Option<String>,
}

impl Decoder {
    /// Creates a decoder for a `width` × `height` desktop sent in `format`.
    #[must_use]
    pub fn new(width: u16, height: u16, format: PixelFormat, limits: Limits) -> Self {
        Self {
            framebuffer: Framebuffer::new(width, height),
            converter: PixelConverter::new(format),
            limits,
            zrle: ZlibStream::default(),
            tight: Default::default(),
            cursor: None,
            cursor_generation: 0,
            screens: Vec::new(),
            extended_resize: false,
            last_resize: None,
            name: None,
        }
    }

    /// The desktop name the server last announced, once.
    pub fn take_name(&mut self) -> Option<String> {
        self.name.take()
    }

    fn desktop_name(&mut self, input: &[u8]) -> Result<Option<usize>, DecodeError> {
        let mut r = Reader::new(input);
        let len = usize::try_from(need!(r.u32())).unwrap_or(usize::MAX);
        if len > crate::encode::MAX_DESKTOP_NAME {
            return Err(DecodeError::LimitExceeded {
                what: "desktop name",
                len: len as u64,
                max: crate::encode::MAX_DESKTOP_NAME as u64,
            });
        }
        let bytes = need!(r.bytes(len));
        self.name = Some(String::from_utf8_lossy(bytes).into_owned());
        Ok(Some(r.consumed()))
    }

    /// The decoded screen.
    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }

    /// The cursor shape last sent by the server (Cursor or `CursorWithAlpha`
    /// pseudo-encoding), or `None` if it has not sent one.
    #[must_use]
    pub fn cursor(&self) -> Option<&Cursor> {
        self.cursor.as_ref()
    }

    /// Increases every time the cursor shape changes, so the UI can tell
    /// when to re-upload it.
    #[must_use]
    pub fn cursor_generation(&self) -> u64 {
        self.cursor_generation
    }

    /// The screen layout from the last applied `ExtendedDesktopSize` rectangle.
    #[must_use]
    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    /// Whether the server supports `SetDesktopSize` (it has sent at least
    /// one `ExtendedDesktopSize` rectangle).
    #[must_use]
    pub fn supports_extended_resize(&self) -> bool {
        self.extended_resize
    }

    /// Reason and status of the last `ExtendedDesktopSize` rectangle, so the
    /// UI can report a refused resize.
    #[must_use]
    pub fn last_resize(&self) -> Option<DesktopResize> {
        self.last_resize
    }

    /// Switches to the pixel format just requested with `SetPixelFormat`.
    pub fn set_pixel_format(&mut self, format: PixelFormat) {
        self.converter = PixelConverter::new(format);
    }

    /// Applies colour-map entries sent by the server.
    pub fn set_colour_map(&mut self, first: u16, colours: &[[u16; 3]]) {
        self.converter.set_colour_map(first, colours);
    }

    fn check_size(&self, width: u16, height: u16) -> Result<(), DecodeError> {
        let max = self.limits.max_framebuffer_side;
        if width > max || height > max {
            return Err(DecodeError::LimitExceeded {
                what: "framebuffer side",
                len: u64::from(width.max(height)),
                max: u64::from(max),
            });
        }
        Ok(())
    }

    fn resize(&mut self, width: u16, height: u16) -> Result<Option<usize>, DecodeError> {
        self.check_size(width, height)?;
        self.framebuffer.resize(width, height);
        Ok(Some(0))
    }

    fn extended_desktop_size(
        &mut self,
        header: &RectHeader,
        input: &[u8],
    ) -> Result<Option<usize>, DecodeError> {
        let Some((change, used)) = desktop::parse(header, input)? else {
            return Ok(None);
        };
        if change.result.applied() {
            self.check_size(change.width, change.height)?;
            let size = (self.framebuffer.width(), self.framebuffer.height());
            // Keep the picture when only the layout changed.
            if size != (change.width, change.height) {
                self.framebuffer.resize(change.width, change.height);
            }
            self.screens = change.screens;
        }
        self.extended_resize = true;
        self.last_resize = Some(change.result);
        Ok(Some(used))
    }

    fn set_cursor(&mut self, decoded: Option<(Cursor, usize)>) -> Option<usize> {
        let (cursor, used) = decoded?;
        self.cursor = Some(cursor);
        self.cursor_generation = self.cursor_generation.wrapping_add(1);
        Some(used)
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
        let src_x = need!(r.u16());
        let src_y = need!(r.u16());
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
        let rect = header.rect;
        match header.encoding {
            Encoding::DesktopSize => self.resize(rect.width, rect.height),
            Encoding::ExtendedDesktopSize => self.extended_desktop_size(header, input),
            Encoding::Raw => self.raw(header, input),
            Encoding::CopyRect => self.copy_rect(header, input),
            Encoding::Rre => {
                crate::rre::decode(&mut self.framebuffer, &self.converter, rect, input)
            }
            Encoding::DesktopName => self.desktop_name(input),
            Encoding::Hextile => {
                hextile::decode(&mut self.framebuffer, &self.converter, rect, input)
            }
            Encoding::Zrle => zrle::decode(
                &mut self.framebuffer,
                &mut self.zrle,
                &self.converter,
                rect,
                input,
            ),
            Encoding::Tight => tight::decode(
                &mut self.framebuffer,
                &mut self.tight,
                &self.converter,
                rect,
                input,
            ),
            Encoding::Cursor => {
                let decoded = cursor::decode_cursor(&self.converter, header, input)?;
                Ok(self.set_cursor(decoded))
            }
            Encoding::CursorWithAlpha => {
                let decoded = cursor::decode_alpha_cursor(header, input)?;
                Ok(self.set_cursor(decoded))
            }
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

    fn rect_header(x: u16, y: u16, w: u16, h: u16, encoding: Encoding) -> RectHeader {
        RectHeader {
            rect: Rect::new(x, y, w, h),
            encoding,
        }
    }

    #[test]
    fn cursor_rects_update_the_cursor_and_generation() {
        let mut d = decoder();
        assert!(d.cursor().is_none());
        let header = rect_header(1, 1, 2, 1, Encoding::Cursor);
        let payload = [1, 2, 3, 0, 4, 5, 6, 0, 0b0100_0000];
        assert_eq!(d.decode_rect(&header, &payload[..8]), Ok(None));
        assert!(d.cursor().is_none());
        assert_eq!(d.cursor_generation(), 0);
        assert_eq!(d.decode_rect(&header, &payload), Ok(Some(9)));
        let cursor = d.cursor().unwrap();
        assert_eq!(cursor.rgba(), [0, 0, 0, 0, 6, 5, 4, 255]);
        assert_eq!(cursor.hotspot(), (1, 0));
        assert_eq!(d.cursor_generation(), 1);
        let alpha = rect_header(0, 0, 1, 1, Encoding::CursorWithAlpha);
        assert_eq!(
            d.decode_rect(&alpha, &[0, 0, 0, 0, 9, 8, 7, 255]),
            Ok(Some(8))
        );
        assert_eq!(d.cursor().unwrap().rgba(), [9, 8, 7, 255]);
        assert_eq!(d.cursor_generation(), 2);
        // Cursor rectangles do not touch the framebuffer.
        assert!(d.framebuffer().pixels().iter().all(|&p| p == 0));
    }

    #[test]
    fn extended_desktop_size_resizes_and_records_layout() {
        let mut d = decoder();
        assert!(!d.supports_extended_resize());
        let screen = Screen {
            id: 1,
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            flags: 0,
        };
        let mut payload = vec![1, 0, 0, 0];
        screen.encode(&mut payload);
        let header = rect_header(0, 0, 640, 480, Encoding::ExtendedDesktopSize);
        assert_eq!(d.decode_rect(&header, &payload[..19]), Ok(None));
        assert!(!d.supports_extended_resize());
        assert_eq!(d.framebuffer().width(), 4);
        assert_eq!(d.decode_rect(&header, &payload), Ok(Some(20)));
        assert!(d.supports_extended_resize());
        assert_eq!(
            (d.framebuffer().width(), d.framebuffer().height()),
            (640, 480)
        );
        assert_eq!(d.screens(), [screen]);
        assert!(d.last_resize().unwrap().applied());
    }

    #[test]
    fn refused_resize_keeps_the_framebuffer_and_reports_the_status() {
        let mut d = decoder();
        let header = rect_header(1, 1, 9999, 9999, Encoding::ExtendedDesktopSize);
        assert_eq!(d.decode_rect(&header, &[0, 0, 0, 0]), Ok(Some(4)));
        assert_eq!(d.framebuffer().width(), 4);
        let result = d.last_resize().unwrap();
        assert_eq!(result.reason, crate::ResizeReason::ThisClient);
        assert_eq!(result.status, crate::ResizeStatus::Prohibited);
        assert!(result.is_failure());
        assert!(d.supports_extended_resize());
    }

    #[test]
    fn extended_desktop_size_respects_limits() {
        let mut d = decoder();
        let header = rect_header(0, 0, 60000, 10, Encoding::ExtendedDesktopSize);
        assert!(matches!(
            d.decode_rect(&header, &[0, 0, 0, 0]),
            Err(DecodeError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn layout_only_changes_keep_the_picture() {
        let mut d = decoder();
        d.decode_rect(&rect_header(0, 0, 1, 1, Encoding::Raw), &[9, 0, 0, 0])
            .unwrap();
        let header = rect_header(0, 0, 4, 4, Encoding::ExtendedDesktopSize);
        assert_eq!(d.decode_rect(&header, &[0, 0, 0, 0]), Ok(Some(4)));
        assert_eq!(d.framebuffer().get(0, 0), Some(9));
    }

    #[test]
    fn new_encodings_are_dispatched() {
        let mut d = decoder();
        // Hextile solid tile.
        let header = rect_header(0, 0, 2, 2, Encoding::Hextile);
        assert_eq!(d.decode_rect(&header, &[2, 7, 0, 0, 0]), Ok(Some(5)));
        assert_eq!(d.framebuffer().get(1, 1), Some(7));
        // Tight fill.
        let header = rect_header(2, 2, 2, 2, Encoding::Tight);
        assert_eq!(d.decode_rect(&header, &[0x80, 0, 0, 5]), Ok(Some(4)));
        assert_eq!(d.framebuffer().get(3, 3), Some(5));
        // ZRLE solid tile.
        let chunk = crate::inflate::test_support::Deflater::default().chunk(&[1, 3, 0, 0]);
        let mut payload = u32::try_from(chunk.len()).unwrap().to_be_bytes().to_vec();
        payload.extend(chunk);
        let header = rect_header(0, 2, 2, 2, Encoding::Zrle);
        assert_eq!(d.decode_rect(&header, &payload), Ok(Some(payload.len())));
        assert_eq!(d.framebuffer().get(0, 3), Some(3));
    }

    proptest::proptest! {
        #[test]
        fn decode_never_panics_for_any_encoding(
            code in proptest::prop_oneof![
                proptest::prelude::Just(0i32), proptest::prelude::Just(1), proptest::prelude::Just(5),
                proptest::prelude::Just(7), proptest::prelude::Just(16), proptest::prelude::Just(-223),
                proptest::prelude::Just(-239), proptest::prelude::Just(-308), proptest::prelude::Just(-314),
            ],
            x in 0u16..40, y in 0u16..40, w in 0u16..40, h in 0u16..40,
            bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
        ) {
            let mut d = Decoder::new(32, 32, PixelFormat::RGB888, Limits::DEFAULT);
            let header = rect_header(x, y, w, h, Encoding::from_code(code));
            if let Ok(Some(used)) = d.decode_rect(&header, &bytes) {
                proptest::prop_assert!(used <= bytes.len());
            }
        }
    }
}
