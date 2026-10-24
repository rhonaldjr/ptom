//! Per-viewer update scheduling: the viewer's pixel format and encodings,
//! its outstanding update request, its accumulated damage, and building
//! `FramebufferUpdate` messages from the [`Desktop`].

use farsight_codec::encode::{self, TIGHT_MAX_AREA, TightEncoder, ZrleEncoder};
use farsight_codec::{Cursor, PixelConverter};
use farsight_rfb::server::ServerConnection;
use farsight_rfb::{
    DecodeError, Encoding, PixelFormat, ProtocolError, Rect, RectHeader, ServerMessage,
};

use crate::Desktop;
use crate::damage::{self, Region};
use crate::desktop::Changes;

/// Largest side of one encoded rectangle. 256 × 256 is Tight's area limit
/// and keeps the work per rectangle bounded for every encoding.
pub(crate) const PIECE: u16 = 256;
/// Most pixel rectangles in one update; the rest wait for the next request.
pub(crate) const MAX_PIECES: usize = 1024;
/// Compression level when the viewer does not ask for one.
const DEFAULT_COMPRESS_LEVEL: u8 = 2;

/// `ExtendedDesktopSize` reasons (rectangle x-position).
const REASON_SERVER: u16 = 0;
const REASON_THIS_CLIENT: u16 = 1;
/// `ExtendedDesktopSize` status "no error".
pub(crate) const STATUS_OK: u16 = 0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One flag per pseudo-encoding.
struct Capabilities {
    cursor: bool,
    cursor_with_alpha: bool,
    desktop_size: bool,
    extended_desktop_size: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Request {
    incremental: bool,
    area: Rect,
}

/// One viewer's view of the desktop.
#[allow(clippy::struct_excessive_bools)] // Independent pending-work flags.
pub(crate) struct UpdateState {
    allowed: Vec<Encoding>,
    converter: PixelConverter,
    encoding: Encoding,
    caps: Capabilities,
    compress_level: u8,
    zrle: Option<ZrleEncoder>,
    tight: Option<TightEncoder>,
    request: Option<Request>,
    forced: Region,
    damage: Region,
    max_rects: usize,
    frame_serial: u64,
    size_serial: u64,
    cursor_serial: u64,
    client_size: (u16, u16),
    size_changed: bool,
    cursor_dirty: bool,
    announce_layout: bool,
    resize_reply: Option<u16>,
}

impl UpdateState {
    /// State for a viewer that was just sent `ServerInit` for `desktop`.
    pub(crate) fn new(allowed: Vec<Encoding>, max_rects: usize, desktop: &Desktop) -> Self {
        let fb = desktop.framebuffer();
        Self {
            allowed,
            converter: PixelConverter::new(PixelFormat::RGB888),
            encoding: Encoding::Raw,
            caps: Capabilities::default(),
            compress_level: DEFAULT_COMPRESS_LEVEL,
            zrle: None,
            tight: None,
            request: None,
            forced: Region::new(max_rects),
            damage: Region::new(max_rects),
            max_rects,
            frame_serial: desktop.frame_serial(),
            size_serial: desktop.size_serial(),
            cursor_serial: desktop.cursor_serial(),
            client_size: (fb.width(), fb.height()),
            size_changed: false,
            cursor_dirty: desktop.cursor().is_some(),
            announce_layout: false,
            resize_reply: None,
        }
    }

    /// The pixel encoding in use.
    pub(crate) fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// The framebuffer size the viewer believes in.
    pub(crate) fn client_size(&self) -> (u16, u16) {
        self.client_size
    }

    /// Whether the viewer may send `SetDesktopSize`.
    pub(crate) fn supports_extended_resize(&self) -> bool {
        self.caps.extended_desktop_size
    }

    /// Applies `SetPixelFormat`. Colour-map formats get a fixed 3-3-2
    /// colour map (sent now), so every encoder still works on true colour.
    pub(crate) fn set_pixel_format(
        &mut self,
        format: PixelFormat,
        connection: &mut ServerConnection,
    ) -> Result<(), ProtocolError> {
        if format.true_colour {
            self.converter = PixelConverter::new(format);
        } else {
            self.converter = PixelConverter::new(PixelFormat {
                true_colour: true,
                // Depth equal to the pixel size keeps the compact pixel forms
                // identical to the viewer's colour-map interpretation.
                depth: format.bits_per_pixel,
                red_max: 7,
                green_max: 7,
                blue_max: 3,
                red_shift: 0,
                green_shift: 3,
                blue_shift: 6,
                ..format
            });
            connection.send(&ServerMessage::SetColourMapEntries {
                first: 0,
                colours: bgr233_colour_map(),
            })?;
        }
        // Cursor pixels are in the viewer's format.
        if self.caps.cursor && !self.caps.cursor_with_alpha {
            self.cursor_dirty = true;
        }
        Ok(())
    }

    /// Applies `SetEncodings`: picks the first of the agent's allowed
    /// encodings that the viewer accepts (Raw otherwise) and notes the
    /// pseudo-encodings and levels.
    pub(crate) fn set_encodings(&mut self, list: &[Encoding]) {
        self.encoding = self
            .allowed
            .iter()
            .copied()
            .find(|e| list.contains(e))
            .unwrap_or(Encoding::Raw);
        let had_layout = self.caps.extended_desktop_size;
        let had_cursor = self.caps.cursor || self.caps.cursor_with_alpha;
        self.caps = Capabilities {
            cursor: list.contains(&Encoding::Cursor),
            cursor_with_alpha: list.contains(&Encoding::CursorWithAlpha),
            desktop_size: list.contains(&Encoding::DesktopSize),
            extended_desktop_size: list.contains(&Encoding::ExtendedDesktopSize),
        };
        if self.caps.extended_desktop_size && !had_layout {
            self.announce_layout = true;
        }
        if (self.caps.cursor || self.caps.cursor_with_alpha) && !had_cursor {
            self.cursor_dirty = true;
        }
        if let Some(level) = list.iter().find_map(|e| match e {
            Encoding::CompressLevel(level) => Some(*level),
            _ => None,
        }) {
            self.compress_level = level;
            if let Some(tight) = &mut self.tight {
                tight.set_compress_level(level);
            }
        }
    }

    /// Records a `FramebufferUpdateRequest`.
    pub(crate) fn request(&mut self, incremental: bool, rect: Rect) {
        let area = rect;
        if !incremental {
            self.forced.add(area);
        }
        self.request = Some(match self.request {
            Some(previous) => Request {
                incremental: previous.incremental && incremental,
                area: if previous.area.is_empty() {
                    area
                } else if area.is_empty() {
                    previous.area
                } else {
                    damage::bounding(previous.area, area)
                },
            },
            None => Request { incremental, area },
        });
    }

    /// Queues the reply to a `SetDesktopSize` request.
    pub(crate) fn reply_to_resize(&mut self, status: u16) {
        self.resize_reply = Some(status);
    }

    /// Takes in what changed on the desktop since the last call.
    pub(crate) fn sync(&mut self, desktop: &Desktop) {
        if desktop.size_serial() != self.size_serial {
            self.size_serial = desktop.size_serial();
            self.size_changed = true;
        }
        if desktop.frame_serial() != self.frame_serial {
            match desktop.changes_since(self.frame_serial) {
                Changes::Rects(rects) => self.damage.add_all(rects),
                Changes::Everything => {
                    let fb = desktop.framebuffer();
                    self.damage.add(Rect::new(0, 0, fb.width(), fb.height()));
                }
            }
            self.frame_serial = desktop.frame_serial();
        }
        if desktop.cursor_serial() != self.cursor_serial {
            self.cursor_serial = desktop.cursor_serial();
            self.cursor_dirty = true;
        }
    }

    fn can_resize(&self) -> bool {
        self.caps.extended_desktop_size || self.caps.desktop_size
    }

    fn sends_cursor(&self) -> bool {
        self.caps.cursor || self.caps.cursor_with_alpha
    }

    fn bounds(&self, desktop: &Desktop) -> Rect {
        let fb = desktop.framebuffer();
        Rect::new(
            0,
            0,
            self.client_size.0.min(fb.width()),
            self.client_size.1.min(fb.height()),
        )
    }

    /// Whether an update should be sent now.
    fn ready(&self, desktop: &Desktop) -> bool {
        let Some(request) = self.request else {
            return false;
        };
        let fb = desktop.framebuffer();
        let resize_pending =
            self.size_changed && self.can_resize() && self.client_size != (fb.width(), fb.height());
        !request.incremental
            || resize_pending
            || self.resize_reply.is_some()
            || (self.announce_layout && self.caps.extended_desktop_size)
            || (self.cursor_dirty && self.sends_cursor())
            || damage::intersect(request.area, self.bounds(desktop))
                .is_some_and(|area| !self.damage.within(area).is_empty())
    }

    /// Sends a `FramebufferUpdate` if a request is outstanding and there is
    /// something to send (or the request was non-incremental).
    pub(crate) fn flush(
        &mut self,
        desktop: &Desktop,
        connection: &mut ServerConnection,
    ) -> Result<(), ProtocolError> {
        if !self.ready(desktop) {
            return Ok(());
        }
        let Some(request) = self.request.take() else {
            return Ok(());
        };
        let (mut rects, resized) = self.pseudo_rects(desktop);
        let bounds = self.bounds(desktop);
        let mut areas = Region::new(self.max_rects);
        if resized {
            self.forced.clear();
            self.damage.clear();
            areas.add(bounds);
        } else {
            areas.add_all(
                self.forced
                    .take()
                    .into_iter()
                    .filter_map(|r| damage::intersect(r, bounds)),
            );
            if let Some(area) = damage::intersect(request.area, bounds) {
                areas.add_all(self.damage.within(area));
                self.damage.subtract(area);
            }
        }
        let mut pieces = Vec::new();
        for area in areas.take() {
            for piece in damage::split(area, PIECE, PIECE) {
                if pieces.len() < MAX_PIECES {
                    pieces.push(piece);
                } else {
                    self.damage.add(piece);
                }
            }
        }
        for piece in pieces {
            let (encoding, payload) = self.encode_piece(desktop, piece)?;
            rects.push((
                RectHeader {
                    rect: piece,
                    encoding,
                },
                payload,
            ));
        }
        // At most a few pseudo-rectangles plus MAX_PIECES: fits in u16.
        let count = u16::try_from(rects.len())
            .map_err(|_| ProtocolError::InvalidState("too many rectangles in one update"))?;
        connection.send(&ServerMessage::FramebufferUpdate { rects: count })?;
        for (header, payload) in &rects {
            connection.send_rect(header, payload)?;
        }
        Ok(())
    }

    /// Size, layout and cursor pseudo-rectangles due in this update, and
    /// whether the viewer's framebuffer size changed (it then needs a full
    /// repaint).
    fn pseudo_rects(&mut self, desktop: &Desktop) -> (Vec<(RectHeader, Vec<u8>)>, bool) {
        let mut rects = Vec::new();
        let fb = desktop.framebuffer();
        let desktop_size = (fb.width(), fb.height());
        let before = self.client_size;
        if std::mem::take(&mut self.size_changed)
            && self.can_resize()
            && self.client_size != desktop_size
        {
            self.client_size = desktop_size;
            rects.push(self.size_rect(desktop, REASON_SERVER, STATUS_OK));
            self.announce_layout = false;
        }
        if let Some(status) = self.resize_reply.take() {
            if status == STATUS_OK {
                self.client_size = desktop_size;
            }
            rects.push(self.size_rect(desktop, REASON_THIS_CLIENT, status));
            self.announce_layout = false;
        }
        if std::mem::take(&mut self.announce_layout) && self.caps.extended_desktop_size {
            // Tells the viewer that SetDesktopSize is understood (same size).
            rects.push(self.size_rect(desktop, REASON_SERVER, STATUS_OK));
        }
        if self.cursor_dirty && self.sends_cursor() {
            self.cursor_dirty = false;
            rects.push(self.cursor_rect(desktop.cursor()));
        }
        (rects, before != self.client_size)
    }

    fn size_rect(&self, desktop: &Desktop, reason: u16, status: u16) -> (RectHeader, Vec<u8>) {
        let (width, height) = self.client_size;
        if self.caps.extended_desktop_size {
            (
                RectHeader {
                    rect: Rect::new(reason, status, width, height),
                    encoding: Encoding::ExtendedDesktopSize,
                },
                encode::extended_desktop_size(&desktop.screens()),
            )
        } else {
            (
                RectHeader {
                    rect: Rect::new(0, 0, width, height),
                    encoding: Encoding::DesktopSize,
                },
                Vec::new(),
            )
        }
    }

    fn cursor_rect(&self, cursor: Option<&Cursor>) -> (RectHeader, Vec<u8>) {
        let hidden = Cursor::from_rgba(0, 0, (0, 0), Vec::new());
        let Some(cursor) = cursor.or(hidden.as_ref()) else {
            // Unreachable: a 0×0 cursor is always valid.
            return (
                RectHeader {
                    rect: Rect::default(),
                    encoding: Encoding::Cursor,
                },
                Vec::new(),
            );
        };
        let (x, y) = cursor.hotspot();
        let rect = Rect::new(x, y, cursor.width(), cursor.height());
        if self.caps.cursor_with_alpha {
            (
                RectHeader {
                    rect,
                    encoding: Encoding::CursorWithAlpha,
                },
                encode::cursor_with_alpha(cursor),
            )
        } else {
            (
                RectHeader {
                    rect,
                    encoding: Encoding::Cursor,
                },
                encode::cursor(cursor, &self.converter),
            )
        }
    }

    fn encode_piece(
        &mut self,
        desktop: &Desktop,
        rect: Rect,
    ) -> Result<(Encoding, Vec<u8>), DecodeError> {
        let fb = desktop.framebuffer();
        let level = self.compress_level;
        match self.encoding {
            Encoding::Tight if rect.area() <= TIGHT_MAX_AREA => Ok((
                Encoding::Tight,
                self.tight
                    .get_or_insert_with(|| TightEncoder::new(level))
                    .encode(fb, rect, &self.converter)?,
            )),
            Encoding::Zrle => Ok((
                Encoding::Zrle,
                self.zrle
                    .get_or_insert_with(|| ZrleEncoder::new(level))
                    .encode(fb, rect, &self.converter)?,
            )),
            _ => Ok((Encoding::Raw, encode::raw(fb, rect, &self.converter)?)),
        }
    }
}

/// The fixed colour map for colour-map viewers: index bits `bbgggrrr`.
fn bgr233_colour_map() -> Vec<[u16; 3]> {
    let scale = |value: u16, max: u16| value * (u16::MAX / max);
    (0..256u16)
        .map(|i| [scale(i & 7, 7), scale((i >> 3) & 7, 7), scale(i >> 6, 3)])
        .collect()
}
