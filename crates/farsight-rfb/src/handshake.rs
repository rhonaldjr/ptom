//! Handshake and initialisation messages (RFC 6143 §7.1–§7.3).

use crate::wire::{Reader, WriteExt, wire_length};
use crate::{DecodeError, Limits, PixelFormat};

/// The `ServerInit` message: framebuffer geometry, native pixel format and
/// desktop name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInit {
    /// Framebuffer width.
    pub width: u16,
    /// Framebuffer height.
    pub height: u16,
    /// The server's natural pixel format.
    pub pixel_format: PixelFormat,
    /// Desktop name (UTF-8 in practice; invalid sequences are replaced).
    pub name: String,
}

impl ServerInit {
    /// Appends the wire encoding to `out`.
    ///
    /// # Errors
    /// [`DecodeError::LimitExceeded`] if the name is longer than `u32::MAX`.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), DecodeError> {
        out.put_u16(self.width);
        out.put_u16(self.height);
        out.extend_from_slice(&self.pixel_format.encode());
        out.put_u32(wire_length("desktop name", self.name.len())?);
        out.extend_from_slice(self.name.as_bytes());
        Ok(())
    }

    /// Decodes from the start of `input`.
    ///
    /// # Errors
    /// [`DecodeError::Truncated`] when more input is needed, or a protocol
    /// violation (bad pixel format, oversized name or framebuffer).
    pub fn decode(input: &[u8], limits: &Limits) -> Result<(Self, usize), DecodeError> {
        let mut r = Reader::new(input);
        let width = r.u16()?;
        let height = r.u16()?;
        let pixel_format = PixelFormat::decode(r.bytes(16)?)?;
        let name_len = r.length("desktop name", limits.max_name)?;
        let name = String::from_utf8_lossy(r.bytes(name_len)?).into_owned();
        if width > limits.max_framebuffer_side || height > limits.max_framebuffer_side {
            return Err(DecodeError::LimitExceeded {
                what: "framebuffer side",
                len: u64::from(width.max(height)),
                max: u64::from(limits.max_framebuffer_side),
            });
        }
        Ok((
            Self {
                width,
                height,
                pixel_format,
                name,
            },
            r.consumed(),
        ))
    }
}

/// Reads a `u32`-length-prefixed failure reason.
pub(crate) fn read_reason(r: &mut Reader<'_>, limits: &Limits) -> Result<String, DecodeError> {
    let len = r.length("reason", limits.max_reason)?;
    Ok(String::from_utf8_lossy(r.bytes(len)?).into_owned())
}

/// Appends a `u32`-length-prefixed failure reason.
pub(crate) fn put_reason(out: &mut Vec<u8>, reason: &str) {
    // Reasons are produced locally and short; cap defensively rather than fail.
    let bytes = reason.as_bytes().get(..4096).unwrap_or(reason.as_bytes());
    out.put_u32(u32::try_from(bytes.len()).unwrap_or(0));
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_init_round_trips() {
        let init = ServerInit {
            width: 1920,
            height: 1080,
            pixel_format: PixelFormat::RGB888,
            name: "pop-os:1".into(),
        };
        let mut out = Vec::new();
        init.encode(&mut out).unwrap();
        assert_eq!(out.len(), 24 + 8);
        assert_eq!(
            ServerInit::decode(&out, &Limits::DEFAULT),
            Ok((init, out.len()))
        );
    }

    #[test]
    fn oversized_framebuffer_is_rejected() {
        let init = ServerInit {
            width: 20000,
            height: 10,
            pixel_format: PixelFormat::RGB888,
            name: String::new(),
        };
        let mut out = Vec::new();
        init.encode(&mut out).unwrap();
        assert!(matches!(
            ServerInit::decode(&out, &Limits::DEFAULT),
            Err(DecodeError::LimitExceeded {
                what: "framebuffer side",
                ..
            })
        ));
    }
}
