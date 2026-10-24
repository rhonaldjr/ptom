//! Tight encoder (rfbproto "Tight Encoding"): solid fill, the palette filter
//! for up to 16 colours (1-bit packing for two), basic compression with the
//! copy filter over four persistent zlib streams, and JPEG for photo-like
//! areas when the client asked for a JPEG quality.

use farsight_rfb::{DecodeError, PixelFormat, Rect};

use super::palette::Palette;
use crate::compact::CompactPixel;
use crate::deflate::ZlibCompressor;
use crate::tight::{MAX_WIDTH, STREAMS};
use crate::{Framebuffer, PixelConverter};

/// Largest rectangle (in pixels) the encoder accepts; split bigger areas.
pub const TIGHT_MAX_AREA: usize = 65_536;
/// Widest rectangle the encoder accepts (the decoder limit).
pub const TIGHT_MAX_WIDTH: u16 = MAX_WIDTH;

/// Filtered data shorter than this is sent without zlib (spec rule).
const MIN_TO_COMPRESS: usize = 12;
/// Largest value the 3-byte compact length can hold.
const MAX_COMPACT_LENGTH: usize = 0x3f_ffff;
/// Most colours sent with the palette filter.
const MAX_PALETTE: usize = 16;

const CONTROL_FILL: u8 = 0x80;
const CONTROL_JPEG: u8 = 0x90;
/// JPEG quality (percent) for each client quality level 0–9.
const JPEG_QUALITY: [u8; 10] = [15, 25, 35, 45, 55, 65, 75, 82, 90, 95];
/// "Read a filter id" flag in the compression-control high nibble.
const EXPLICIT_FILTER: u8 = 0x4;
const FILTER_PALETTE: u8 = 1;

/// Stream used for full-colour (copy filter) data.
const STREAM_FULL: usize = 0;
/// Stream used for two-colour (1-bit) palette data.
const STREAM_MONO: usize = 1;
/// Stream used for indexed palette data.
const STREAM_INDEXED: usize = 2;

/// Encodes rectangles as Tight for one client. The zlib streams persist for
/// the connection, so use one encoder per client and never share it.
#[derive(Debug)]
pub struct TightEncoder {
    level: u8,
    streams: [Option<ZlibCompressor>; STREAMS],
    needs_reset: [bool; STREAMS],
}

impl TightEncoder {
    /// Creates an encoder at zlib `level` (0 fastest to 9 smallest).
    #[must_use]
    pub fn new(level: u8) -> Self {
        Self {
            level,
            streams: Default::default(),
            needs_reset: [false; STREAMS],
        }
    }

    /// Changes the zlib level. Streams already in use are restarted (the
    /// client is told to reset them with the next rectangle that uses them).
    pub fn set_compress_level(&mut self, level: u8) {
        if level == self.level {
            return;
        }
        self.level = level;
        for (stream, reset) in self.streams.iter_mut().zip(&mut self.needs_reset) {
            if stream.take().is_some() {
                *reset = true;
            }
        }
    }

    /// Encodes `rect` of `framebuffer` in the client's format.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the
    /// framebuffer; [`DecodeError::LimitExceeded`] if it is wider than
    /// [`TIGHT_MAX_WIDTH`] or larger than [`TIGHT_MAX_AREA`].
    pub fn encode(
        &mut self,
        framebuffer: &Framebuffer,
        rect: Rect,
        converter: &PixelConverter,
    ) -> Result<Vec<u8>, DecodeError> {
        framebuffer.check(rect)?;
        check_limits(rect)?;
        let tpixel = CompactPixel::tpixel(converter);
        let raw: Vec<u32> = framebuffer
            .read(rect)?
            .into_iter()
            .map(|rgb| converter.to_raw(rgb))
            .collect();
        let mut out = Vec::new();
        match Palette::collect(&raw, MAX_PALETTE) {
            Some(palette) if palette.len() == 1 => {
                out.push(CONTROL_FILL | self.take_resets());
                tpixel.write(palette.colour(0), &mut out);
            }
            Some(palette) if palette.len() >= 2 => {
                let stream = if palette.len() == 2 {
                    STREAM_MONO
                } else {
                    STREAM_INDEXED
                };
                let data = palette_indices(&raw, usize::from(rect.width), &palette);
                out.push(self.basic_control(stream, true));
                out.push(FILTER_PALETTE);
                // 2..=16 colours.
                out.push(u8::try_from(palette.len() - 1).unwrap_or(0));
                for &colour in palette.colours() {
                    tpixel.write(colour, &mut out);
                }
                self.put_data(stream, &data, &mut out)?;
            }
            _ => {
                let mut data = Vec::with_capacity(raw.len() * tpixel.len());
                for &pixel in &raw {
                    tpixel.write(pixel, &mut data);
                }
                out.push(self.basic_control(STREAM_FULL, false));
                self.put_data(STREAM_FULL, &data, &mut out)?;
            }
        }
        Ok(out)
    }

    /// Encodes `rect` as a Tight JPEG rectangle at client quality `level`
    /// (0–9). Returns `None` when the client's pixel format can't take
    /// JPEG (it must be true colour with 8 bits per channel).
    ///
    /// # Errors
    /// As for [`Self::encode`].
    pub fn encode_jpeg(
        &mut self,
        framebuffer: &Framebuffer,
        rect: Rect,
        converter: &PixelConverter,
        level: u8,
    ) -> Result<Option<Vec<u8>>, DecodeError> {
        framebuffer.check(rect)?;
        check_limits(rect)?;
        if !jpeg_capable(converter.format()) {
            return Ok(None);
        }
        let mut rgb = Vec::with_capacity(rect.area() * 3);
        for pixel in framebuffer.read(rect)? {
            let [_, r, g, b] = pixel.to_be_bytes();
            rgb.extend_from_slice(&[r, g, b]);
        }
        let quality = JPEG_QUALITY
            .get(usize::from(level))
            .copied()
            .unwrap_or(JPEG_QUALITY[9]);
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, quality)
            .encode(&rgb, rect.width, rect.height, jpeg_encoder::ColorType::Rgb)
            .map_err(|_| DecodeError::Malformed("JPEG encoding failed"))?;
        let mut out = vec![CONTROL_JPEG | self.take_resets()];
        put_compact_length(&mut out, jpeg.len())?;
        out.extend_from_slice(&jpeg);
        Ok(Some(out))
    }

    /// Reset bits for every stream restarted since it was last used. Fill
    /// rectangles carry them too, so pending resets go out early.
    fn take_resets(&mut self) -> u8 {
        let mut bits = 0;
        for (bit, reset) in self.needs_reset.iter_mut().enumerate() {
            if std::mem::take(reset) {
                bits |= 1 << bit;
            }
        }
        bits
    }

    fn basic_control(&mut self, stream: usize, explicit_filter: bool) -> u8 {
        let filter = if explicit_filter { EXPLICIT_FILTER } else { 0 };
        // `stream` < 4.
        let stream = u8::try_from(stream).unwrap_or(0);
        ((filter | stream) << 4) | self.take_resets()
    }

    fn put_data(
        &mut self,
        stream: usize,
        data: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), DecodeError> {
        if data.len() < MIN_TO_COMPRESS {
            out.extend_from_slice(data);
            return Ok(());
        }
        let level = self.level;
        let compressor = self
            .streams
            .get_mut(stream)
            .ok_or(DecodeError::Malformed("Tight stream out of range"))?
            .get_or_insert_with(|| ZlibCompressor::new(level));
        let compressed = compressor.compress(data)?;
        put_compact_length(out, compressed.len())?;
        out.extend_from_slice(&compressed);
        Ok(())
    }
}

/// Whether a client in `format` can take Tight JPEG: true colour, 32 bits
/// a pixel, 8 bits per channel (rfbproto "Tight Encoding").
#[must_use]
pub fn jpeg_capable(format: PixelFormat) -> bool {
    format.true_colour
        && format.bits_per_pixel == 32
        && [format.red_max, format.green_max, format.blue_max] == [255, 255, 255]
}

fn check_limits(rect: Rect) -> Result<(), DecodeError> {
    if rect.width > TIGHT_MAX_WIDTH {
        return Err(DecodeError::LimitExceeded {
            what: "Tight rectangle width",
            len: u64::from(rect.width),
            max: u64::from(TIGHT_MAX_WIDTH),
        });
    }
    if rect.area() > TIGHT_MAX_AREA {
        return Err(DecodeError::LimitExceeded {
            what: "Tight rectangle area",
            len: rect.area() as u64,
            max: TIGHT_MAX_AREA as u64,
        });
    }
    Ok(())
}

/// Palette-filtered data: 1 bit per pixel (rows padded to a byte) for two
/// colours, otherwise one index byte per pixel.
fn palette_indices(raw: &[u32], width: usize, palette: &Palette) -> Vec<u8> {
    if palette.len() != 2 {
        return raw.iter().map(|&p| palette.index(p)).collect();
    }
    let width = width.max(1);
    let mut out = Vec::with_capacity(raw.len().div_ceil(8) + raw.len() / width);
    for row in raw.chunks(width) {
        for byte in row.chunks(8) {
            let bits = byte
                .iter()
                .enumerate()
                .fold(0u8, |acc, (i, &p)| acc | (palette.index(p) << (7 - i)));
            out.push(bits);
        }
    }
    out
}

/// Writes the Tight compact length: 7 + 7 + 8 bits, least significant first.
fn put_compact_length(out: &mut Vec<u8>, len: usize) -> Result<(), DecodeError> {
    if len > MAX_COMPACT_LENGTH {
        return Err(DecodeError::LimitExceeded {
            what: "Tight compressed data",
            len: len as u64,
            max: MAX_COMPACT_LENGTH as u64,
        });
    }
    let low = |shift: u32| u8::try_from((len >> shift) & 0x7f).unwrap_or(0);
    if len < 0x80 {
        out.push(low(0));
    } else if len < 0x4000 {
        out.extend([low(0) | 0x80, low(7)]);
    } else {
        out.extend([
            low(0) | 0x80,
            low(7) | 0x80,
            u8::try_from(len >> 14).unwrap_or(0),
        ]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decoder;
    use farsight_rfb::client::RectDecoder;
    use farsight_rfb::{Encoding, Limits, PixelFormat, RectHeader};

    fn decode(d: &mut Decoder, rect: Rect, payload: &[u8]) {
        let header = RectHeader {
            rect,
            encoding: Encoding::Tight,
        };
        assert_eq!(d.decode_rect(&header, payload), Ok(Some(payload.len())));
    }

    #[test]
    fn compact_length_boundaries() {
        for (len, bytes) in [
            (0, &[0x00][..]),
            (127, &[0x7f][..]),
            (128, &[0x80, 0x01][..]),
            (10_000, &[0x90, 0x4e][..]),
            (16_384, &[0x80, 0x80, 0x01][..]),
            (MAX_COMPACT_LENGTH, &[0xff, 0xff, 0xff][..]),
        ] {
            let mut out = Vec::new();
            put_compact_length(&mut out, len).unwrap();
            assert_eq!(out, bytes, "{len}");
        }
        assert!(put_compact_length(&mut Vec::new(), MAX_COMPACT_LENGTH + 1).is_err());
    }

    #[test]
    fn fill_mono_indexed_and_full_colour_round_trip_across_a_level_change() {
        let mut fb = Framebuffer::new(40, 30);
        fb.write(Rect::new(0, 0, 40, 30), |i| {
            let (x, y) = (i % 40, i / 40);
            let v = u32::try_from(x * 31 + y * 17).unwrap();
            match x / 10 {
                0 => 0x0000_8000,
                1 => {
                    if (x + y) % 3 == 0 {
                        0xff_ffff
                    } else {
                        0
                    }
                }
                2 => (v % 9) * 0x0011_1111,
                _ => v.wrapping_mul(2_654_435_761) & 0xff_ffff,
            }
        })
        .unwrap();
        let conv = PixelConverter::new(PixelFormat::RGB888);
        let mut enc = TightEncoder::new(1);
        let mut d = Decoder::new(40, 30, PixelFormat::RGB888, Limits::DEFAULT);
        for round in 0..3 {
            if round == 2 {
                enc.set_compress_level(9);
            }
            for x in [0, 10, 20, 30] {
                let rect = Rect::new(x, 0, 10, 30);
                decode(&mut d, rect, &enc.encode(&fb, rect, &conv).unwrap());
            }
            assert_eq!(d.framebuffer(), &fb, "round {round}");
        }
        // Tiny rectangles skip zlib entirely.
        let tiny = Rect::new(30, 0, 2, 1);
        decode(&mut d, tiny, &enc.encode(&fb, tiny, &conv).unwrap());
    }

    #[test]
    fn oversized_rectangles_are_refused() {
        let fb = Framebuffer::new(4096, 64);
        let conv = PixelConverter::new(PixelFormat::RGB888);
        let mut enc = TightEncoder::new(6);
        assert!(enc.encode(&fb, Rect::new(0, 0, 2049, 1), &conv).is_err());
        assert!(enc.encode(&fb, Rect::new(0, 0, 2048, 33), &conv).is_err());
        assert!(enc.encode(&fb, Rect::new(0, 0, 2048, 32), &conv).is_ok());
    }
}
