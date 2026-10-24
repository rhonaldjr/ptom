//! Normal-phase messages (RFC 6143 §7.5 client-to-server, §7.6 server-to-client).
//!
//! Decoders take the start of the unread input and return the message and
//! the number of bytes consumed, or [`DecodeError::Truncated`] when the
//! message is not complete yet. Clipboard payloads larger than
//! [`Limits::max_cut_text`] are reported as [`CutText::Oversized`] so the
//! caller can skip them without buffering.

use crate::wire::{Reader, WriteExt, wire_length};
use crate::{DecodeError, Encoding, Limits, PixelFormat, Rect, Screen};

/// Client-to-server message type codes.
mod client_type {
    pub const SET_PIXEL_FORMAT: u8 = 0;
    pub const SET_ENCODINGS: u8 = 2;
    pub const FRAMEBUFFER_UPDATE_REQUEST: u8 = 3;
    pub const KEY_EVENT: u8 = 4;
    pub const POINTER_EVENT: u8 = 5;
    pub const CLIENT_CUT_TEXT: u8 = 6;
    pub const SET_DESKTOP_SIZE: u8 = 251;
}

/// Server-to-client message type codes.
mod server_type {
    pub const FRAMEBUFFER_UPDATE: u8 = 0;
    pub const SET_COLOUR_MAP_ENTRIES: u8 = 1;
    pub const BELL: u8 = 2;
    pub const SERVER_CUT_TEXT: u8 = 3;
}

/// Clipboard text carried by `ClientCutText` / `ServerCutText` (ISO 8859-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutText {
    /// The text, as Latin-1 bytes.
    Text(Vec<u8>),
    /// The peer announced more than the configured limit. The caller must
    /// skip `len` payload bytes that follow the header.
    Oversized {
        /// Payload length still to be skipped.
        len: u32,
    },
}

impl CutText {
    /// Converts Latin-1 bytes to a `String` (every byte maps to one char).
    #[must_use]
    pub fn latin1_to_string(bytes: &[u8]) -> String {
        bytes.iter().copied().map(char::from).collect()
    }

    /// Converts text to Latin-1, replacing unrepresentable characters with `?`.
    #[must_use]
    pub fn string_to_latin1(text: &str) -> Vec<u8> {
        text.chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
            .collect()
    }
}

/// A message from the viewer to the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// Change the pixel format of subsequent updates.
    SetPixelFormat(PixelFormat),
    /// Encodings the client accepts, most preferred first.
    SetEncodings(Vec<Encoding>),
    /// Ask for (part of) the framebuffer.
    FramebufferUpdateRequest {
        /// Only send changes since the last update.
        incremental: bool,
        /// Region of interest.
        rect: Rect,
    },
    /// Key press or release, identified by X11 keysym.
    KeyEvent {
        /// True for press, false for release.
        down: bool,
        /// X11 keysym.
        keysym: u32,
    },
    /// Pointer position and button mask (bit 0 = left … bit 7).
    PointerEvent {
        /// Bitmask of pressed buttons.
        buttons: u8,
        /// Horizontal position.
        x: u16,
        /// Vertical position.
        y: u16,
    },
    /// Clipboard contents from the viewer.
    ClientCutText(CutText),
    /// Ask the server to change the desktop size and screen layout. Only
    /// valid once the server has sent an `ExtendedDesktopSize` rectangle.
    SetDesktopSize {
        /// Requested framebuffer width.
        width: u16,
        /// Requested framebuffer height.
        height: u16,
        /// Requested screen layout (at most 255 screens).
        screens: Vec<Screen>,
    },
}

impl ClientMessage {
    /// Appends the wire encoding to `out`.
    ///
    /// # Errors
    /// [`DecodeError::LimitExceeded`] if a list or text is too long for the
    /// wire format.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), DecodeError> {
        match self {
            Self::SetPixelFormat(format) => {
                out.put_u8(client_type::SET_PIXEL_FORMAT);
                out.put_padding(3);
                out.extend_from_slice(&format.encode());
            }
            Self::SetEncodings(encodings) => {
                let count =
                    u16::try_from(encodings.len()).map_err(|_| DecodeError::LimitExceeded {
                        what: "encodings",
                        len: encodings.len() as u64,
                        max: u64::from(u16::MAX),
                    })?;
                out.put_u8(client_type::SET_ENCODINGS);
                out.put_padding(1);
                out.put_u16(count);
                for encoding in encodings {
                    out.put_i32(encoding.code());
                }
            }
            Self::FramebufferUpdateRequest { incremental, rect } => {
                out.put_u8(client_type::FRAMEBUFFER_UPDATE_REQUEST);
                out.put_u8(u8::from(*incremental));
                put_rect(out, *rect);
            }
            Self::KeyEvent { down, keysym } => {
                out.put_u8(client_type::KEY_EVENT);
                out.put_u8(u8::from(*down));
                out.put_padding(2);
                out.put_u32(*keysym);
            }
            Self::PointerEvent { buttons, x, y } => {
                out.put_u8(client_type::POINTER_EVENT);
                out.put_u8(*buttons);
                out.put_u16(*x);
                out.put_u16(*y);
            }
            Self::ClientCutText(text) => {
                out.put_u8(client_type::CLIENT_CUT_TEXT);
                out.put_padding(3);
                put_cut_text(out, text)?;
            }
            Self::SetDesktopSize {
                width,
                height,
                screens,
            } => {
                let count =
                    u8::try_from(screens.len()).map_err(|_| DecodeError::LimitExceeded {
                        what: "screens",
                        len: screens.len() as u64,
                        max: u64::from(u8::MAX),
                    })?;
                out.put_u8(client_type::SET_DESKTOP_SIZE);
                out.put_padding(1);
                out.put_u16(*width);
                out.put_u16(*height);
                out.put_u8(count);
                out.put_padding(1);
                for screen in screens {
                    screen.encode(out);
                }
            }
        }
        Ok(())
    }

    /// Decodes one message from the start of `input`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] when more input is needed; any other
    /// variant is a protocol violation.
    pub fn decode(input: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(input);
        let message = match r.u8()? {
            client_type::SET_PIXEL_FORMAT => {
                r.skip(3)?;
                Self::SetPixelFormat(PixelFormat::decode(r.bytes(16)?)?)
            }
            client_type::SET_ENCODINGS => {
                r.skip(1)?;
                let count = r.u16()?;
                if count > limits.max_encodings {
                    return Err(DecodeError::LimitExceeded {
                        what: "encodings",
                        len: u64::from(count),
                        max: u64::from(limits.max_encodings),
                    });
                }
                let mut encodings = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    encodings.push(Encoding::from_code(r.i32()?));
                }
                Self::SetEncodings(encodings)
            }
            client_type::FRAMEBUFFER_UPDATE_REQUEST => {
                let incremental = r.u8()? != 0;
                Self::FramebufferUpdateRequest {
                    incremental,
                    rect: read_rect(&mut r)?,
                }
            }
            client_type::KEY_EVENT => {
                let down = r.u8()? != 0;
                r.skip(2)?;
                Self::KeyEvent {
                    down,
                    keysym: r.u32()?,
                }
            }
            client_type::POINTER_EVENT => Self::PointerEvent {
                buttons: r.u8()?,
                x: r.u16()?,
                y: r.u16()?,
            },
            client_type::CLIENT_CUT_TEXT => {
                r.skip(3)?;
                Self::ClientCutText(read_cut_text(&mut r, limits)?)
            }
            client_type::SET_DESKTOP_SIZE => {
                r.skip(1)?;
                let width = r.u16()?;
                let height = r.u16()?;
                let count = r.u8()?;
                r.skip(1)?;
                let mut screens = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    screens.push(Screen::decode(&mut r)?);
                }
                Self::SetDesktopSize {
                    width,
                    height,
                    screens,
                }
            }
            other => return Err(DecodeError::UnknownMessageType(other)),
        };
        Ok((message, r.consumed()))
    }
}

/// A message from the server to the viewer, other than rectangle payloads.
///
/// A framebuffer update is decoded as its header only; the rectangles that
/// follow are read with [`RectHeader::decode`] and an encoding-specific decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// Start of a framebuffer update with this many rectangles
    /// (`0xFFFF` means "until a `LastRect` pseudo-rectangle").
    FramebufferUpdate {
        /// Number of rectangles that follow.
        rects: u16,
    },
    /// Colour-map entries, as 16-bit RGB triples starting at `first`.
    SetColourMapEntries {
        /// Index of the first entry.
        first: u16,
        /// RGB values.
        colours: Vec<[u16; 3]>,
    },
    /// Ring the bell.
    Bell,
    /// Clipboard contents from the server.
    ServerCutText(CutText),
}

/// `rects` value meaning "terminated by a `LastRect` rectangle".
pub const RECTS_UNTIL_LAST_RECT: u16 = 0xFFFF;

impl ServerMessage {
    /// Appends the wire encoding to `out`.
    ///
    /// # Errors
    /// [`DecodeError::LimitExceeded`] if a list or text is too long for the
    /// wire format.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), DecodeError> {
        match self {
            Self::FramebufferUpdate { rects } => {
                out.put_u8(server_type::FRAMEBUFFER_UPDATE);
                out.put_padding(1);
                out.put_u16(*rects);
            }
            Self::SetColourMapEntries { first, colours } => {
                let count =
                    u16::try_from(colours.len()).map_err(|_| DecodeError::LimitExceeded {
                        what: "colour map",
                        len: colours.len() as u64,
                        max: u64::from(u16::MAX),
                    })?;
                out.put_u8(server_type::SET_COLOUR_MAP_ENTRIES);
                out.put_padding(1);
                out.put_u16(*first);
                out.put_u16(count);
                for [r, g, b] in colours {
                    out.put_u16(*r);
                    out.put_u16(*g);
                    out.put_u16(*b);
                }
            }
            Self::Bell => out.put_u8(server_type::BELL),
            Self::ServerCutText(text) => {
                out.put_u8(server_type::SERVER_CUT_TEXT);
                out.put_padding(3);
                put_cut_text(out, text)?;
            }
        }
        Ok(())
    }

    /// Decodes one message (or update header) from the start of `input`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] when more input is needed; any other
    /// variant is a protocol violation.
    pub fn decode(input: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(input);
        let message = match r.u8()? {
            server_type::FRAMEBUFFER_UPDATE => {
                r.skip(1)?;
                Self::FramebufferUpdate { rects: r.u16()? }
            }
            server_type::SET_COLOUR_MAP_ENTRIES => {
                r.skip(1)?;
                let first = r.u16()?;
                let count = r.u16()?;
                if u32::from(first) + u32::from(count) > 1 << 16 {
                    return Err(DecodeError::Malformed(
                        "colour map entries past index 65535",
                    ));
                }
                let mut colours = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    colours.push([r.u16()?, r.u16()?, r.u16()?]);
                }
                Self::SetColourMapEntries { first, colours }
            }
            server_type::BELL => Self::Bell,
            server_type::SERVER_CUT_TEXT => {
                r.skip(3)?;
                Self::ServerCutText(read_cut_text(&mut r, limits)?)
            }
            other => return Err(DecodeError::UnknownMessageType(other)),
        };
        Ok((message, r.consumed()))
    }
}

/// The header preceding each rectangle in a framebuffer update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectHeader {
    /// Area covered (for pseudo-encodings the meaning is encoding-specific).
    pub rect: Rect,
    /// How the payload is encoded.
    pub encoding: Encoding,
}

/// Size in bytes of an encoded [`RectHeader`].
pub const RECT_HEADER_LEN: usize = 12;

impl RectHeader {
    /// Appends the wire encoding to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        put_rect(out, self.rect);
        out.put_i32(self.encoding.code());
    }

    /// Decodes a header from the start of `input`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] when fewer than 12 bytes are available.
    pub fn decode(input: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(input);
        let rect = read_rect(&mut r)?;
        Ok(Self {
            rect,
            encoding: Encoding::from_code(r.i32()?),
        })
    }
}

fn put_rect(out: &mut Vec<u8>, rect: Rect) {
    out.put_u16(rect.x);
    out.put_u16(rect.y);
    out.put_u16(rect.width);
    out.put_u16(rect.height);
}

fn read_rect(r: &mut Reader<'_>) -> Result<Rect, DecodeError> {
    Ok(Rect::new(r.u16()?, r.u16()?, r.u16()?, r.u16()?))
}

fn put_cut_text(out: &mut Vec<u8>, text: &CutText) -> Result<(), DecodeError> {
    match text {
        CutText::Text(bytes) => {
            out.put_u32(wire_length("cut text", bytes.len())?);
            out.extend_from_slice(bytes);
            Ok(())
        }
        CutText::Oversized { .. } => Err(DecodeError::Malformed("cannot encode skipped cut text")),
    }
}

fn read_cut_text(r: &mut Reader<'_>, limits: &Limits) -> Result<CutText, DecodeError> {
    let len = r.i32()?;
    // A negative length is the Extended Clipboard format, which is only valid
    // once that pseudo-encoding has been negotiated (roadmap M9).
    let len = u32::try_from(len)
        .map_err(|_| DecodeError::Malformed("extended clipboard message without negotiation"))?;
    if len > limits.max_cut_text {
        return Ok(CutText::Oversized { len });
    }
    Ok(CutText::Text(r.bytes(len as usize)?.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn round_trip_client(message: &ClientMessage) {
        let mut out = Vec::new();
        message.encode(&mut out).unwrap();
        assert_eq!(
            ClientMessage::decode(&out, &Limits::DEFAULT),
            Ok((message.clone(), out.len()))
        );
        for cut in 0..out.len() {
            assert!(
                ClientMessage::decode(&out[..cut], &Limits::DEFAULT)
                    .unwrap_err()
                    .is_incomplete()
            );
        }
    }

    #[test]
    fn client_messages_round_trip_and_report_truncation() {
        for message in [
            ClientMessage::SetPixelFormat(PixelFormat::RGB888),
            ClientMessage::SetEncodings(vec![
                Encoding::Tight,
                Encoding::Raw,
                Encoding::JpegQuality(8),
            ]),
            ClientMessage::FramebufferUpdateRequest {
                incremental: true,
                rect: Rect::new(1, 2, 3, 4),
            },
            ClientMessage::KeyEvent {
                down: true,
                keysym: 0xff0d,
            },
            ClientMessage::PointerEvent {
                buttons: 0b101,
                x: 640,
                y: 480,
            },
            ClientMessage::ClientCutText(CutText::Text(b"hello".to_vec())),
            ClientMessage::SetDesktopSize {
                width: 3840,
                height: 1080,
                screens: vec![
                    Screen {
                        id: 1,
                        x: 0,
                        y: 0,
                        width: 1920,
                        height: 1080,
                        flags: 0,
                    },
                    Screen {
                        id: 2,
                        x: 1920,
                        y: 0,
                        width: 1920,
                        height: 1080,
                        flags: 0x8000_0001,
                    },
                ],
            },
            ClientMessage::SetDesktopSize {
                width: 0,
                height: 0,
                screens: vec![],
            },
        ] {
            round_trip_client(&message);
        }
    }

    #[test]
    fn key_event_matches_rfc_layout() {
        let mut out = Vec::new();
        ClientMessage::KeyEvent {
            down: true,
            keysym: 0x61,
        }
        .encode(&mut out)
        .unwrap();
        assert_eq!(out, [4, 1, 0, 0, 0, 0, 0, 0x61]);
    }

    #[test]
    fn set_desktop_size_matches_spec_layout() {
        let mut out = Vec::new();
        ClientMessage::SetDesktopSize {
            width: 0x0400,
            height: 0x0300,
            screens: vec![Screen {
                id: 7,
                x: 1,
                y: 2,
                width: 0x0400,
                height: 0x0300,
                flags: 9,
            }],
        }
        .encode(&mut out)
        .unwrap();
        let header = [251, 0, 0x04, 0x00, 0x03, 0x00, 1, 0];
        let screen = [0, 0, 0, 7, 0, 1, 0, 2, 0x04, 0x00, 0x03, 0x00, 0, 0, 0, 9];
        assert_eq!(out, [header.as_slice(), screen.as_slice()].concat());
    }

    #[test]
    fn set_desktop_size_with_too_many_screens_cannot_be_encoded() {
        let message = ClientMessage::SetDesktopSize {
            width: 1,
            height: 1,
            screens: vec![Screen::default(); 256],
        };
        assert!(matches!(
            message.encode(&mut Vec::new()),
            Err(DecodeError::LimitExceeded {
                what: "screens",
                ..
            })
        ));
    }

    #[test]
    fn oversized_cut_text_is_reported_without_waiting_for_payload() {
        let limits = Limits {
            max_cut_text: 4,
            ..Limits::DEFAULT
        };
        let header = [6, 0, 0, 0, 0, 0, 0, 100];
        assert_eq!(
            ClientMessage::decode(&header, &limits),
            Ok((
                ClientMessage::ClientCutText(CutText::Oversized { len: 100 }),
                8
            ))
        );
    }

    #[test]
    fn too_many_encodings_is_rejected_before_reading_them() {
        let limits = Limits {
            max_encodings: 2,
            ..Limits::DEFAULT
        };
        assert!(matches!(
            ClientMessage::decode(&[2, 0, 0, 3], &limits),
            Err(DecodeError::LimitExceeded {
                what: "encodings",
                ..
            })
        ));
    }

    #[test]
    fn negative_cut_text_length_is_rejected() {
        assert!(matches!(
            ClientMessage::decode(&[6, 0, 0, 0, 0xff, 0xff, 0xff, 0xf0], &Limits::DEFAULT),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn server_messages_round_trip() {
        for message in [
            ServerMessage::FramebufferUpdate { rects: 3 },
            ServerMessage::SetColourMapEntries {
                first: 4,
                colours: vec![[1, 2, 3], [65535, 0, 9]],
            },
            ServerMessage::Bell,
            ServerMessage::ServerCutText(CutText::Text(b"clip".to_vec())),
        ] {
            let mut out = Vec::new();
            message.encode(&mut out).unwrap();
            assert_eq!(
                ServerMessage::decode(&out, &Limits::DEFAULT),
                Ok((message.clone(), out.len()))
            );
        }
    }

    #[test]
    fn colour_map_overflowing_index_space_is_rejected() {
        assert!(matches!(
            ServerMessage::decode(&[1, 0, 0xff, 0xff, 0, 2], &Limits::DEFAULT),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn latin1_conversion_round_trips_representable_text() {
        let bytes = CutText::string_to_latin1("café €");
        assert_eq!(bytes, b"caf\xe9 ?");
        assert_eq!(CutText::latin1_to_string(b"caf\xe9"), "café");
    }

    proptest! {
        #[test]
        fn client_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = ClientMessage::decode(&bytes, &Limits::DEFAULT);
        }

        #[test]
        fn server_decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = ServerMessage::decode(&bytes, &Limits::DEFAULT);
        }

        #[test]
        fn decoded_client_messages_reencode_identically(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            if let Ok((message, used)) = ClientMessage::decode(&bytes, &Limits::DEFAULT) {
                let mut out = Vec::new();
                if message.encode(&mut out).is_ok() {
                    prop_assert_eq!(ClientMessage::decode(&out, &Limits::DEFAULT), Ok((message, out.len())));
                }
                prop_assert!(used <= bytes.len());
            }
        }
    }
}
