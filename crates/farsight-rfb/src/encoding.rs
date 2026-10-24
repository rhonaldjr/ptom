//! Encoding and pseudo-encoding identifiers (RFC 6143 §7.7, rfbproto registry).

/// An encoding type as sent in `SetEncodings` and rectangle headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// Uncompressed pixels.
    Raw,
    /// Copy from elsewhere in the framebuffer.
    CopyRect,
    /// Rise-and-run-length encoding.
    Rre,
    /// 16×16 tiles with sub-encodings.
    Hextile,
    /// Zlib streams with palette, gradient and JPEG sub-encodings.
    Tight,
    /// Zlib run-length encoding over 64×64 tiles.
    Zrle,
    /// Standalone JPEG rectangles.
    Jpeg,
    /// H.264 (TigerVNC's identifier).
    H264,
    /// Pseudo: the framebuffer was resized.
    DesktopSize,
    /// Pseudo: no more rectangles in this update.
    LastRect,
    /// Pseudo: cursor shape with a bitmask.
    Cursor,
    /// Pseudo: resize with screen layout (multi-monitor).
    ExtendedDesktopSize,
    /// Pseudo: the desktop name changed.
    DesktopName,
    /// Pseudo: Fence synchronisation messages.
    Fence,
    /// Pseudo: continuous updates.
    ContinuousUpdates,
    /// Pseudo: cursor shape with an alpha channel.
    CursorWithAlpha,
    /// Pseudo: QEMU extended (scancode) key events.
    QemuExtendedKeyEvent,
    /// Pseudo: keyboard LED state.
    LedState,
    /// Pseudo: extended clipboard (formats, large transfers).
    ExtendedClipboard,
    /// Pseudo: extended mouse buttons (back/forward).
    ExtendedMouseButtons,
    /// Pseudo: JPEG quality level 0 (worst) to 9 (best).
    JpegQuality(u8),
    /// Pseudo: compression level 0 (fastest) to 9 (smallest).
    CompressLevel(u8),
    /// Any identifier Farsight does not recognise.
    Unknown(i32),
}

const JPEG_QUALITY_BASE: i32 = -32;
const COMPRESS_LEVEL_BASE: i32 = -256;
const EXTENDED_CLIPBOARD: i32 = 0xC0A1_E5CE_u32.cast_signed();
const H264: i32 = 0x4832_3634;

impl Encoding {
    /// Maps a wire identifier to an encoding.
    #[must_use]
    pub fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Raw,
            1 => Self::CopyRect,
            2 => Self::Rre,
            5 => Self::Hextile,
            7 => Self::Tight,
            16 => Self::Zrle,
            21 => Self::Jpeg,
            H264 => Self::H264,
            -223 => Self::DesktopSize,
            -224 => Self::LastRect,
            -239 => Self::Cursor,
            -258 => Self::QemuExtendedKeyEvent,
            -261 => Self::LedState,
            -307 => Self::DesktopName,
            -308 => Self::ExtendedDesktopSize,
            -312 => Self::Fence,
            -313 => Self::ContinuousUpdates,
            -314 => Self::CursorWithAlpha,
            -316 => Self::ExtendedMouseButtons,
            EXTENDED_CLIPBOARD => Self::ExtendedClipboard,
            -32..=-23 => Self::JpegQuality(level(code, JPEG_QUALITY_BASE)),
            -256..=-247 => Self::CompressLevel(level(code, COMPRESS_LEVEL_BASE)),
            other => Self::Unknown(other),
        }
    }

    /// The wire identifier.
    #[must_use]
    pub fn code(self) -> i32 {
        match self {
            Self::Raw => 0,
            Self::CopyRect => 1,
            Self::Rre => 2,
            Self::Hextile => 5,
            Self::Tight => 7,
            Self::Zrle => 16,
            Self::Jpeg => 21,
            Self::H264 => H264,
            Self::DesktopSize => -223,
            Self::LastRect => -224,
            Self::Cursor => -239,
            Self::QemuExtendedKeyEvent => -258,
            Self::LedState => -261,
            Self::DesktopName => -307,
            Self::ExtendedDesktopSize => -308,
            Self::Fence => -312,
            Self::ContinuousUpdates => -313,
            Self::CursorWithAlpha => -314,
            Self::ExtendedMouseButtons => -316,
            Self::ExtendedClipboard => EXTENDED_CLIPBOARD,
            Self::JpegQuality(level) => JPEG_QUALITY_BASE + i32::from(level.min(9)),
            Self::CompressLevel(level) => COMPRESS_LEVEL_BASE + i32::from(level.min(9)),
            Self::Unknown(code) => code,
        }
    }

    /// Whether this is a pseudo-encoding (a capability, not pixel data).
    #[must_use]
    pub fn is_pseudo(self) -> bool {
        !matches!(
            self,
            Self::Raw
                | Self::CopyRect
                | Self::Rre
                | Self::Hextile
                | Self::Tight
                | Self::Zrle
                | Self::Jpeg
                | Self::H264
                | Self::Unknown(_)
        )
    }
}

fn level(code: i32, base: i32) -> u8 {
    u8::try_from(code - base).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn codes_round_trip(code in any::<i32>()) {
            prop_assert_eq!(Encoding::from_code(code).code(), code);
        }
    }

    #[test]
    fn quality_and_compression_levels_map_to_ranges() {
        assert_eq!(Encoding::from_code(-23), Encoding::JpegQuality(9));
        assert_eq!(Encoding::from_code(-256), Encoding::CompressLevel(0));
        assert_eq!(Encoding::JpegQuality(6).code(), -26);
    }
}
