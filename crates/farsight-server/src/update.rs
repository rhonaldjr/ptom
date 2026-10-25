//! Per-viewer update scheduling: the viewer's pixel format and encodings,
//! its outstanding update request, its accumulated damage, and building
//! `FramebufferUpdate` messages from the [`Desktop`].

use farsight_codec::encode::{self, TIGHT_MAX_AREA, TightEncoder, ZrleEncoder};
use farsight_codec::{Cursor, PixelConverter};
use farsight_rfb::messages::fence;
use farsight_rfb::server::ServerConnection;
use farsight_rfb::{
    DecodeError, Encoding, PixelFormat, ProtocolError, Rect, RectHeader, ServerMessage,
};

use crate::Desktop;
use crate::damage::{self, Region};
use crate::desktop::Changes;
use crate::select::{self, Choice, Content, Offer};

/// Largest side of one encoded rectangle. 256 × 256 is Tight's area limit
/// and keeps the work per rectangle bounded for every encoding.
pub(crate) const PIECE: u16 = 256;
/// Most pixel rectangles in one update; the rest wait for the next request.
pub(crate) const MAX_PIECES: usize = 1024;
/// Compression level when the viewer does not ask for one.
const DEFAULT_COMPRESS_LEVEL: u8 = 2;

/// Updates sent in continuous mode before the viewer must acknowledge one
/// (through a fence): the congestion window.
const MAX_IN_FLIGHT: u8 = 2;
/// Marks fences the agent sent, so their replies can be told apart.
const FENCE_TAG: [u8; 4] = *b"FSGT";
/// RRE is used only while it needs fewer subrectangles than area / this.
const RRE_DENSITY: usize = 16;

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
    desktop_name: bool,
    fence: bool,
    continuous_updates: bool,
    qemu_keys: bool,
    led_state: bool,
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
    /// Encodings both sides support, the agent's preference first.
    usable: Vec<Encoding>,
    jpeg_quality: Option<u8>,
    /// Continuous updates (rfbproto): the area kept up to date, if on.
    continuous: Option<Rect>,
    /// Updates sent in continuous mode that the viewer hasn't acknowledged.
    in_flight: u8,
    fence_seq: u32,
    /// Sequence numbers of the agent's fences still waiting for a reply.
    outstanding: Vec<u32>,
    /// Fence replies owed to the viewer (sent before anything else).
    fence_replies: Vec<(u32, Vec<u8>)>,
    /// Tell the viewer continuous updates are available (once).
    announce_continuous: bool,
    continuous_ended: bool,
    name_serial: u64,
    name_dirty: bool,
    /// Confirm QEMU extended key events (a pseudo-rectangle, once).
    confirm_qemu_keys: bool,
    led_serial: u64,
    leds_dirty: bool,
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
            usable: vec![Encoding::Raw],
            jpeg_quality: None,
            continuous: None,
            in_flight: 0,
            fence_seq: 0,
            outstanding: Vec::new(),
            fence_replies: Vec::new(),
            announce_continuous: false,
            continuous_ended: false,
            name_serial: desktop.name_serial(),
            name_dirty: false,
            confirm_qemu_keys: false,
            led_serial: desktop.led_serial(),
            leds_dirty: false,
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
        self.usable = self
            .allowed
            .iter()
            .copied()
            .filter(|e| list.contains(e))
            .collect();
        if !self.usable.contains(&Encoding::Raw) {
            self.usable.push(Encoding::Raw);
        }
        self.encoding = self.usable.first().copied().unwrap_or(Encoding::Raw);
        self.jpeg_quality = list.iter().find_map(|e| match e {
            Encoding::JpegQuality(level) => Some(*level),
            _ => None,
        });
        let had_layout = self.caps.extended_desktop_size;
        let had_cursor = self.caps.cursor || self.caps.cursor_with_alpha;
        let had_continuous = self.caps.continuous_updates && self.caps.fence;
        self.caps = Capabilities {
            cursor: list.contains(&Encoding::Cursor),
            cursor_with_alpha: list.contains(&Encoding::CursorWithAlpha),
            desktop_size: list.contains(&Encoding::DesktopSize),
            extended_desktop_size: list.contains(&Encoding::ExtendedDesktopSize),
            desktop_name: list.contains(&Encoding::DesktopName),
            fence: list.contains(&Encoding::Fence),
            continuous_updates: list.contains(&Encoding::ContinuousUpdates),
            qemu_keys: list.contains(&Encoding::QemuExtendedKeyEvent),
            led_state: list.contains(&Encoding::LedState),
        };
        if self.caps.qemu_keys {
            self.confirm_qemu_keys = true;
        }
        if self.caps.led_state {
            self.leds_dirty = true;
        }
        // Continuous updates need fences for flow control; announce them
        // (an unprompted EndOfContinuousUpdates) when both are understood.
        let continuous_now = self.caps.continuous_updates && self.caps.fence;
        if continuous_now && !had_continuous {
            self.announce_continuous = true;
        }
        // The viewer dropped them: stop, so no unknown messages follow.
        if !continuous_now && self.continuous.take().is_some() {
            self.outstanding.clear();
            self.in_flight = 0;
        }
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

    /// Applies `EnableContinuousUpdates`. Ignored unless the viewer also
    /// negotiated fences (the agent needs its acknowledgements).
    pub(crate) fn enable_continuous(&mut self, enable: bool, area: Rect) {
        if !(self.caps.continuous_updates && self.caps.fence) {
            return;
        }
        if enable {
            self.continuous = Some(area);
        } else {
            // Always confirmed, even if they were already off (rfbproto).
            self.continuous = None;
            self.continuous_ended = true;
            self.in_flight = 0;
            self.outstanding.clear();
        }
    }

    /// Handles a viewer's fence: requests are answered (in order, before any
    /// later update); replies to the agent's own fences open the window.
    pub(crate) fn fence(&mut self, flags: u32, payload: Vec<u8>) {
        if !self.caps.fence {
            return;
        }
        if flags & fence::REQUEST != 0 {
            self.fence_replies
                .push((flags & fence::BLOCK_BEFORE, payload));
        } else if let Some(seq) = our_fence(&payload)
            && let Some(index) = self.outstanding.iter().position(|&s| s == seq)
        {
            // Only a reply to a fence actually sent opens the window.
            self.outstanding.swap_remove(index);
            self.in_flight = self.in_flight.saturating_sub(1);
        }
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
        if desktop.name_serial() != self.name_serial {
            self.name_serial = desktop.name_serial();
            self.name_dirty = true;
        }
        if desktop.led_serial() != self.led_serial {
            self.led_serial = desktop.led_serial();
            self.leds_dirty = true;
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

    /// The request to answer: the viewer's, or (in continuous mode, while
    /// the congestion window has room) a standing incremental one.
    fn effective_request(&self) -> Option<Request> {
        self.request.or_else(|| {
            let area = self.continuous?;
            (self.in_flight < MAX_IN_FLIGHT).then_some(Request {
                incremental: true,
                area,
            })
        })
    }

    /// Whether an update should be sent now.
    fn ready(&self, desktop: &Desktop) -> bool {
        let Some(request) = self.effective_request() else {
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
            || (self.name_dirty && self.caps.desktop_name)
            || self.confirm_qemu_keys
            || (self.leds_dirty && self.caps.led_state && desktop.leds().is_some())
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
        for (flags, payload) in std::mem::take(&mut self.fence_replies) {
            connection.send(&ServerMessage::Fence { flags, payload })?;
        }
        if std::mem::take(&mut self.announce_continuous)
            || std::mem::take(&mut self.continuous_ended)
        {
            connection.send(&ServerMessage::EndOfContinuousUpdates)?;
        }
        if !self.ready(desktop) {
            return Ok(());
        }
        let Some(request) = self.request.take().or_else(|| self.effective_request()) else {
            return Ok(());
        };
        let continuous = self.continuous.is_some();
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
        if continuous {
            // The reply to this fence says the viewer has the update.
            self.fence_seq = self.fence_seq.wrapping_add(1);
            let mut payload = FENCE_TAG.to_vec();
            payload.extend_from_slice(&self.fence_seq.to_be_bytes());
            connection.send(&ServerMessage::Fence {
                flags: fence::REQUEST | fence::BLOCK_BEFORE,
                payload,
            })?;
            self.outstanding.push(self.fence_seq);
            self.in_flight = self.in_flight.saturating_add(1);
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
        if std::mem::take(&mut self.confirm_qemu_keys) {
            rects.push((
                RectHeader {
                    rect: Rect::default(),
                    encoding: Encoding::QemuExtendedKeyEvent,
                },
                Vec::new(),
            ));
        }
        if self.leds_dirty
            && self.caps.led_state
            && let Some(leds) = desktop.leds()
        {
            self.leds_dirty = false;
            rects.push((
                RectHeader {
                    rect: Rect::default(),
                    encoding: Encoding::LedState,
                },
                vec![leds],
            ));
        }
        if std::mem::take(&mut self.name_dirty) && self.caps.desktop_name {
            rects.push((
                RectHeader {
                    rect: Rect::default(),
                    encoding: Encoding::DesktopName,
                },
                encode::desktop_name(desktop.name()),
            ));
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
        // With only one usable encoding there's nothing to choose.
        let content = if self.usable.len() > 1 {
            select::analyse(&fb.read(rect)?)
        } else {
            Content::Flat
        };
        let offer = Offer {
            usable: &self.usable,
            jpeg_quality: self.jpeg_quality,
            jpeg_capable: encode::jpeg_capable(self.converter.format()),
        };
        let encoding = match select::choose(content, rect.area(), offer) {
            Choice::TightJpeg(quality) if rect.area() <= TIGHT_MAX_AREA => {
                let tight = self.tight.get_or_insert_with(|| TightEncoder::new(level));
                if let Some(payload) = tight.encode_jpeg(fb, rect, &self.converter, quality)? {
                    return Ok((Encoding::Tight, payload));
                }
                Encoding::Tight
            }
            // Too big for one Tight rectangle (pieces are sized so this can't happen).
            Choice::TightJpeg(_) => self.encoding,
            Choice::Lossless(encoding) => encoding,
        };
        match encoding {
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
            Encoding::Rre => {
                let budget = (rect.area() / RRE_DENSITY).max(1);
                match encode::rre(fb, rect, &self.converter, budget)? {
                    Some(payload) => Ok((Encoding::Rre, payload)),
                    None if self.usable.contains(&Encoding::Hextile) => Ok((
                        Encoding::Hextile,
                        encode::hextile(fb, rect, &self.converter)?,
                    )),
                    None => Ok((Encoding::Raw, encode::raw(fb, rect, &self.converter)?)),
                }
            }
            Encoding::Hextile => Ok((
                Encoding::Hextile,
                encode::hextile(fb, rect, &self.converter)?,
            )),
            _ => Ok((Encoding::Raw, encode::raw(fb, rect, &self.converter)?)),
        }
    }
}

/// The sequence number in a fence payload the agent made (tag + seq).
fn our_fence(payload: &[u8]) -> Option<u32> {
    let seq = payload.strip_prefix(&FENCE_TAG)?;
    Some(u32::from_be_bytes(seq.try_into().ok()?))
}

/// The fixed colour map for colour-map viewers: index bits `bbgggrrr`.
fn bgr233_colour_map() -> Vec<[u16; 3]> {
    let scale = |value: u16, max: u16| value * (u16::MAX / max);
    (0..256u16)
        .map(|i| [scale(i & 7, 7), scale((i >> 3) & 7, 7), scale(i >> 6, 3)])
        .collect()
}
