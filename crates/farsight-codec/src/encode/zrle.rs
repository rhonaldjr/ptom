//! ZRLE encoder (rfbproto "ZRLE Encoding"): 64×64 tiles, each sent as the
//! cheapest of raw, solid, packed palette, plain RLE and palette RLE, all
//! through one zlib stream that lasts for the whole connection.

use farsight_rfb::wire::WriteExt;
use farsight_rfb::{DecodeError, Rect};

use super::palette::{Palette, runs};
use crate::compact::CompactPixel;
use crate::deflate::ZlibCompressor;
use crate::hextile::tiles;
use crate::{Framebuffer, PixelConverter};

const TILE: u16 = 64;
/// Largest palette a palette-RLE tile may carry.
const MAX_RLE_PALETTE: usize = 127;
/// Largest palette a packed-palette tile may carry.
const MAX_PACKED_PALETTE: usize = 16;

const SUB_RAW: u8 = 0;
const SUB_SOLID: u8 = 1;
const SUB_PLAIN_RLE: u8 = 128;

/// Encodes rectangles as ZRLE for one client. The zlib stream persists for
/// the connection, so use one encoder per client and never share it.
#[derive(Debug)]
pub struct ZrleEncoder {
    stream: ZlibCompressor,
}

impl ZrleEncoder {
    /// Creates an encoder at zlib `level` (0 fastest to 9 smallest). ZRLE's
    /// stream cannot be restarted mid-connection, so the level is fixed.
    #[must_use]
    pub fn new(level: u8) -> Self {
        Self {
            stream: ZlibCompressor::new(level),
        }
    }

    /// Encodes `rect` of `framebuffer` in the client's format: a `u32`
    /// length followed by the zlib data.
    ///
    /// # Errors
    /// [`DecodeError::RectOutOfBounds`] if `rect` is not inside the
    /// framebuffer, or [`DecodeError::LimitExceeded`] if the compressed data
    /// does not fit the length field.
    pub fn encode(
        &mut self,
        framebuffer: &Framebuffer,
        rect: Rect,
        converter: &PixelConverter,
    ) -> Result<Vec<u8>, DecodeError> {
        framebuffer.check(rect)?;
        let cpixel = CompactPixel::cpixel(converter);
        let mut data = Vec::with_capacity(rect.area() * cpixel.len() / 4 + 64);
        for tile in tiles(rect, TILE) {
            let raw: Vec<u32> = framebuffer
                .read(tile)?
                .into_iter()
                .map(|rgb| converter.to_raw(rgb))
                .collect();
            encode_tile(&raw, usize::from(tile.width), cpixel, &mut data);
        }
        let compressed = self.stream.compress(&data)?;
        let mut out = Vec::with_capacity(compressed.len() + 4);
        out.put_u32(farsight_rfb::wire::wire_length(
            "ZRLE data",
            compressed.len(),
        )?);
        out.extend_from_slice(&compressed);
        Ok(out)
    }
}

/// Bytes needed for a run-length field of a run of `len` pixels.
fn run_length_bytes(len: usize) -> usize {
    (len - 1) / 255 + 1
}

fn put_run_length(out: &mut Vec<u8>, len: usize) {
    let mut rest = len - 1;
    while rest >= 255 {
        out.push(255);
        rest -= 255;
    }
    // `rest` < 255 here.
    out.push(u8::try_from(rest).unwrap_or(0));
}

fn packed_bits(colours: usize) -> usize {
    match colours {
        0..=2 => 1,
        3 | 4 => 2,
        _ => 4,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileKind {
    Raw,
    Packed,
    PlainRle,
    PaletteRle,
}

/// Appends one tile (raw pixel values, row-major) to `out`.
fn encode_tile(raw: &[u32], width: usize, cpixel: CompactPixel<'_>, out: &mut Vec<u8>) {
    let width = width.max(1);
    let height = raw.len() / width;
    let cp = cpixel.len();
    let palette = Palette::collect(raw, MAX_RLE_PALETTE);
    if let Some(palette) = &palette
        && palette.len() == 1
    {
        out.push(SUB_SOLID);
        cpixel.write(palette.colour(0), out);
        return;
    }
    let runs: Vec<(u32, usize)> = runs(raw).collect();
    let mut best = (TileKind::Raw, raw.len() * cp);
    let mut consider = |kind: TileKind, cost: usize| {
        if cost < best.1 {
            best = (kind, cost);
        }
    };
    consider(
        TileKind::PlainRle,
        runs.iter().map(|&(_, n)| cp + run_length_bytes(n)).sum(),
    );
    if let Some(palette) = &palette {
        let colours = palette.len();
        consider(
            TileKind::PaletteRle,
            colours * cp
                + runs
                    .iter()
                    .map(|&(_, n)| if n == 1 { 1 } else { 1 + run_length_bytes(n) })
                    .sum::<usize>(),
        );
        if colours <= MAX_PACKED_PALETTE {
            let row_bytes = (width * packed_bits(colours)).div_ceil(8);
            consider(TileKind::Packed, colours * cp + height * row_bytes);
        }
    }
    match (best.0, &palette) {
        (TileKind::Packed, Some(palette)) => packed(raw, width, palette, cpixel, out),
        (TileKind::PaletteRle, Some(palette)) => palette_rle(&runs, palette, cpixel, out),
        (TileKind::PlainRle, _) => {
            out.push(SUB_PLAIN_RLE);
            for &(colour, n) in &runs {
                cpixel.write(colour, out);
                put_run_length(out, n);
            }
        }
        _ => {
            out.push(SUB_RAW);
            for &pixel in raw {
                cpixel.write(pixel, out);
            }
        }
    }
}

fn put_palette(palette: &Palette, cpixel: CompactPixel<'_>, out: &mut Vec<u8>) {
    for &colour in palette.colours() {
        cpixel.write(colour, out);
    }
}

fn packed(
    raw: &[u32],
    width: usize,
    palette: &Palette,
    cpixel: CompactPixel<'_>,
    out: &mut Vec<u8>,
) {
    // 2..=16 colours here.
    out.push(u8::try_from(palette.len()).unwrap_or(0));
    put_palette(palette, cpixel, out);
    let bits = packed_bits(palette.len());
    for row in raw.chunks(width) {
        let mut byte = 0u8;
        let mut used = 0;
        for &pixel in row {
            byte = (byte << bits) | palette.index(pixel);
            used += bits;
            if used == 8 {
                out.push(byte);
                byte = 0;
                used = 0;
            }
        }
        if used > 0 {
            out.push(byte << (8 - used));
        }
    }
}

fn palette_rle(
    runs: &[(u32, usize)],
    palette: &Palette,
    cpixel: CompactPixel<'_>,
    out: &mut Vec<u8>,
) {
    // 2..=127 colours here, so the subencoding is 130..=255.
    out.push(128 | u8::try_from(palette.len()).unwrap_or(0));
    put_palette(palette, cpixel, out);
    for &(colour, n) in runs {
        let index = palette.index(colour);
        if n == 1 {
            out.push(index);
        } else {
            out.push(0x80 | index);
            put_run_length(out, n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decoder;
    use farsight_rfb::client::RectDecoder;
    use farsight_rfb::{Encoding, Limits, PixelFormat, RectHeader};

    fn round_trip(fb: &Framebuffer, rect: Rect) -> usize {
        let conv = PixelConverter::new(PixelFormat::RGB888);
        let payload = ZrleEncoder::new(6).encode(fb, rect, &conv).unwrap();
        let mut d = Decoder::new(
            fb.width(),
            fb.height(),
            PixelFormat::RGB888,
            Limits::DEFAULT,
        );
        let header = RectHeader {
            rect,
            encoding: Encoding::Zrle,
        };
        assert_eq!(d.decode_rect(&header, &payload), Ok(Some(payload.len())));
        assert_eq!(d.framebuffer().read(rect), fb.read(rect));
        payload.len()
    }

    #[test]
    fn solid_tiles_are_tiny() {
        let mut fb = Framebuffer::new(128, 128);
        fb.fill(Rect::new(0, 0, 128, 128), 0x0012_3456).unwrap();
        assert!(round_trip(&fb, Rect::new(0, 0, 128, 128)) < 40);
    }

    #[test]
    fn every_subencoding_round_trips() {
        let mut fb = Framebuffer::new(70, 66);
        // Two colours (packed), stripes (RLE), noise (raw), many colours.
        fb.write(Rect::new(0, 0, 70, 66), |i| {
            let (x, y) = (i % 70, i / 70);
            let x32 = u32::try_from(x).unwrap();
            let y32 = u32::try_from(y).unwrap();
            match (x / 20, y / 33) {
                (0, _) => {
                    if (x + y) % 2 == 0 {
                        0xff
                    } else {
                        0xff00
                    }
                }
                (1, 0) => (y32 / 4) * 0x0001_0000,
                (1, 1) => (x32.wrapping_mul(2_654_435_761) ^ y32.wrapping_mul(40503)) & 0xff_ffff,
                _ => (x32 * 7 + y32 * 13) % 90,
            }
        })
        .unwrap();
        round_trip(&fb, Rect::new(0, 0, 70, 66));
        round_trip(&fb, Rect::new(3, 5, 61, 60));
        round_trip(&fb, Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn long_runs_use_continuation_bytes() {
        let mut out = Vec::new();
        put_run_length(&mut out, 256);
        assert_eq!(out, [255, 0]);
        assert_eq!(run_length_bytes(256), 2);
        out.clear();
        put_run_length(&mut out, 255);
        assert_eq!(out, [254]);
        assert_eq!(run_length_bytes(1), 1);
    }
}
