//! ZRLE (encoding 16): one zlib stream per connection carrying 64×64 tiles
//! that are raw, solid, packed-palette or run-length encoded
//! (rfbproto "ZRLE Encoding").

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Rect};

use crate::compact::CompactPixel;
use crate::hextile::tiles;
use crate::inflate::ZlibStream;
use crate::util::{add, complete, mul, need};
use crate::{Framebuffer, PixelConverter};

const TILE: u16 = 64;
const MAX_PALETTE: usize = 127;

/// Decodes a ZRLE payload into `fb`, inflating with the connection's stream.
pub(crate) fn decode(
    fb: &mut Framebuffer,
    stream: &mut ZlibStream,
    converter: &PixelConverter,
    rect: Rect,
    input: &[u8],
) -> Result<Option<usize>, DecodeError> {
    fb.check(rect)?;
    let cpixel = CompactPixel::cpixel(converter);
    let max_inflated = max_inflated(rect, cpixel.len())?;
    // Deflate adds about 5 bytes per 64 KiB stored block plus flush markers;
    // anything far beyond that is not a real encoder's output.
    let max_compressed = max_inflated
        .saturating_add(max_inflated / 64)
        .saturating_add(1024);
    let mut r = Reader::new(input);
    let len = need!(r.u32());
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    if len > max_compressed {
        return Err(DecodeError::LimitExceeded {
            what: "ZRLE data",
            len: len as u64,
            max: max_compressed as u64,
        });
    }
    let compressed = need!(r.bytes(len));
    let data = stream.inflate(compressed, max_inflated)?;
    paint(fb, cpixel, rect, &data)?;
    Ok(Some(r.consumed()))
}

/// The largest valid uncompressed size for `rect`: per tile, the
/// subencoding byte, a full palette, and a pixel plus run byte per pixel.
fn max_inflated(rect: Rect, cpixel: usize) -> Result<usize, DecodeError> {
    let per_tile = 1 + MAX_PALETTE * cpixel;
    let tiles = tiles(rect, TILE).count();
    add(mul(tiles, per_tile)?, mul(rect.area(), cpixel + 1)?)
}

fn paint(
    fb: &mut Framebuffer,
    cpixel: CompactPixel<'_>,
    rect: Rect,
    data: &[u8],
) -> Result<(), DecodeError> {
    let mut r = Reader::new(data);
    let mut scratch = vec![0u32; usize::from(TILE) * usize::from(TILE)];
    for tile in tiles(rect, TILE) {
        let pixels = scratch.get_mut(..tile.area()).unwrap_or_default();
        decode_tile(&mut r, cpixel, usize::from(tile.width), pixels)?;
        fb.write(tile, |i| pixels.get(i).copied().unwrap_or(0))?;
    }
    if r.remaining() == 0 {
        Ok(())
    } else {
        Err(DecodeError::Malformed(
            "unused ZRLE data after the last tile",
        ))
    }
}

fn decode_tile(
    r: &mut Reader<'_>,
    cpixel: CompactPixel<'_>,
    width: usize,
    pixels: &mut [u32],
) -> Result<(), DecodeError> {
    let read_pixel = |r: &mut Reader<'_>| -> Result<u32, DecodeError> {
        Ok(cpixel.rgb(complete(r.bytes(cpixel.len()))?))
    };
    match complete(r.u8())? {
        0 => {
            for slot in pixels.iter_mut() {
                *slot = read_pixel(r)?;
            }
        }
        1 => pixels.fill(read_pixel(r)?),
        size @ 2..=16 => {
            let palette = read_palette(r, cpixel, usize::from(size))?;
            packed(r, &palette, width, pixels)?;
        }
        128 => {
            let mut filled = 0;
            while filled < pixels.len() {
                let colour = read_pixel(r)?;
                let run = run_length(r, pixels.len() - filled)?;
                fill_run(pixels, filled, run, colour);
                filled += run;
            }
        }
        sub @ 130..=255 => {
            let palette = read_palette(r, cpixel, usize::from(sub - 128))?;
            let mut filled = 0;
            while filled < pixels.len() {
                let byte = complete(r.u8())?;
                let colour = *palette
                    .get(usize::from(byte & 0x7f))
                    .ok_or(DecodeError::Malformed("ZRLE palette index out of range"))?;
                let run = if byte & 0x80 == 0 {
                    1
                } else {
                    run_length(r, pixels.len() - filled)?
                };
                fill_run(pixels, filled, run, colour);
                filled += run;
            }
        }
        _ => return Err(DecodeError::Malformed("unused ZRLE subencoding")),
    }
    Ok(())
}

fn read_palette(
    r: &mut Reader<'_>,
    cpixel: CompactPixel<'_>,
    size: usize,
) -> Result<Vec<u32>, DecodeError> {
    let bytes = complete(r.bytes(size * cpixel.len()))?;
    Ok(bytes
        .chunks_exact(cpixel.len())
        .map(|p| cpixel.rgb(p))
        .collect())
}

/// Packed palette indices: 1, 2 or 4 bits each, most significant first,
/// every row padded to a whole byte.
fn packed(
    r: &mut Reader<'_>,
    palette: &[u32],
    width: usize,
    pixels: &mut [u32],
) -> Result<(), DecodeError> {
    let bits = match palette.len() {
        2 => 1,
        3 | 4 => 2,
        _ => 4,
    };
    let row_bytes = (width * bits).div_ceil(8);
    let mask = (1u8 << bits) - 1;
    for row in pixels.chunks_mut(width.max(1)) {
        let bytes = complete(r.bytes(row_bytes))?;
        for (x, slot) in row.iter_mut().enumerate() {
            let bit = x * bits;
            let byte = bytes.get(bit / 8).copied().unwrap_or(0);
            // Fits in u32: `bit % 8 + bits` is at most 8.
            let shift = 8 - bits - bit % 8;
            let index = (byte >> shift) & mask;
            *slot = *palette
                .get(usize::from(index))
                .ok_or(DecodeError::Malformed("ZRLE palette index out of range"))?;
        }
    }
    Ok(())
}

/// Run length: one plus the sum of bytes up to and including the first
/// byte that is not 255. Runs may not exceed `max` pixels.
fn run_length(r: &mut Reader<'_>, max: usize) -> Result<usize, DecodeError> {
    let mut len = 1usize;
    loop {
        let byte = complete(r.u8())?;
        len += usize::from(byte);
        if len > max {
            return Err(DecodeError::Malformed("ZRLE run past the end of the tile"));
        }
        if byte != 255 {
            return Ok(len);
        }
    }
}

fn fill_run(pixels: &mut [u32], start: usize, len: usize, colour: u32) {
    if let Some(run) = pixels.get_mut(start..start + len) {
        run.fill(colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inflate::test_support::Deflater;
    use farsight_rfb::PixelFormat;
    use proptest::prelude::*;

    const RED: u32 = 0x00ff_0000;
    const GREEN: u32 = 0x0000_ff00;
    const BLUE: u32 = 0x0000_00ff;

    /// CPIXEL for RGB888 little-endian: B, G, R.
    fn cp(rgb: u32) -> [u8; 3] {
        let [b, g, r, _] = rgb.to_le_bytes();
        [b, g, r]
    }

    fn wrap(chunk: &[u8]) -> Vec<u8> {
        let mut out = u32::try_from(chunk.len()).unwrap().to_be_bytes().to_vec();
        out.extend_from_slice(chunk);
        out
    }

    struct Session {
        fb: Framebuffer,
        stream: ZlibStream,
        converter: PixelConverter,
    }

    impl Session {
        fn new(w: u16, h: u16) -> Self {
            Self {
                fb: Framebuffer::new(w, h),
                stream: ZlibStream::default(),
                converter: PixelConverter::new(PixelFormat::RGB888),
            }
        }
        fn decode(&mut self, rect: Rect, input: &[u8]) -> Result<Option<usize>, DecodeError> {
            decode(&mut self.fb, &mut self.stream, &self.converter, rect, input)
        }
    }

    #[test]
    fn every_subencoding_decodes() {
        let rect = Rect::new(0, 0, 4, 2);
        let mut deflater = Deflater::default();
        let cases: Vec<(Vec<u8>, [u32; 8])> = vec![
            // Solid.
            ([vec![1], cp(RED).to_vec()].concat(), [RED; 8]),
            // Raw.
            (
                [
                    vec![0],
                    [RED, GREEN, BLUE, RED, GREEN, BLUE, RED, GREEN]
                        .iter()
                        .flat_map(|&c| cp(c))
                        .collect(),
                ]
                .concat(),
                [RED, GREEN, BLUE, RED, GREEN, BLUE, RED, GREEN],
            ),
            // Packed palette of 2 (1 bit): rows 1001, 0110.
            (
                [
                    vec![2],
                    cp(RED).to_vec(),
                    cp(BLUE).to_vec(),
                    vec![0b1001_0000, 0b0110_0000],
                ]
                .concat(),
                [BLUE, RED, RED, BLUE, RED, BLUE, BLUE, RED],
            ),
            // Packed palette of 3 (2 bits): rows 0 1 2 0, 2 2 1 1.
            (
                [
                    vec![3],
                    cp(RED).to_vec(),
                    cp(GREEN).to_vec(),
                    cp(BLUE).to_vec(),
                    vec![0b0001_1000, 0b1010_0101],
                ]
                .concat(),
                [RED, GREEN, BLUE, RED, BLUE, BLUE, GREEN, GREEN],
            ),
            // Packed palette of 5 (4 bits).
            (
                [
                    vec![5],
                    [RED, GREEN, BLUE, RED, GREEN]
                        .iter()
                        .flat_map(|&c| cp(c))
                        .collect(),
                    vec![0x01, 0x23, 0x42, 0x10],
                ]
                .concat(),
                [RED, GREEN, BLUE, RED, GREEN, BLUE, GREEN, RED],
            ),
            // Plain RLE: red ×3 (run byte 2), blue ×5 (run byte 4).
            (
                [
                    vec![128],
                    cp(RED).to_vec(),
                    vec![2],
                    cp(BLUE).to_vec(),
                    vec![4],
                ]
                .concat(),
                [RED, RED, RED, BLUE, BLUE, BLUE, BLUE, BLUE],
            ),
            // Palette RLE of 2: index 1 once, index 0 ×6, index 1 once.
            (
                [
                    vec![130],
                    cp(RED).to_vec(),
                    cp(GREEN).to_vec(),
                    vec![1, 0x80, 5, 1],
                ]
                .concat(),
                [GREEN, RED, RED, RED, RED, RED, RED, GREEN],
            ),
        ];
        let mut session = Session::new(4, 2);
        for (raw, expected) in cases {
            let payload = wrap(&deflater.chunk(&raw));
            assert_eq!(session.decode(rect, &payload), Ok(Some(payload.len())));
            assert_eq!(session.fb.pixels(), expected);
        }
    }

    #[test]
    fn long_runs_use_255_continuation_bytes() {
        // 64×5 = 320 pixels: one run of 320 = 1 + 255 + 64 → [255, 64].
        let raw = [vec![128], cp(BLUE).to_vec(), vec![255, 64]].concat();
        let mut session = Session::new(64, 5);
        let payload = wrap(&Deflater::default().chunk(&raw));
        assert_eq!(
            session.decode(Rect::new(0, 0, 64, 5), &payload),
            Ok(Some(payload.len()))
        );
        assert!(session.fb.pixels().iter().all(|&p| p == BLUE));
    }

    #[test]
    fn multiple_tiles_and_shared_stream_state() {
        // 65×1: a 64-wide tile and a 1-wide tile.
        let raw = [vec![1], cp(RED).to_vec(), vec![1], cp(GREEN).to_vec()].concat();
        let mut deflater = Deflater::default();
        let mut session = Session::new(65, 1);
        let rect = Rect::new(0, 0, 65, 1);
        for _ in 0..3 {
            let payload = wrap(&deflater.chunk(&raw));
            assert_eq!(session.decode(rect, &payload), Ok(Some(payload.len())));
        }
        assert_eq!(session.fb.get(63, 0), Some(RED));
        assert_eq!(session.fb.get(64, 0), Some(GREEN));
    }

    #[test]
    fn truncation_waits_and_leaves_stream_untouched() {
        let mut deflater = Deflater::default();
        let first = wrap(&deflater.chunk(&[1, 1, 2, 3]));
        let second = wrap(&deflater.chunk(&[1, 1, 2, 3]));
        let rect = Rect::new(0, 0, 2, 2);
        for cut in 0..first.len() {
            let mut session = Session::new(2, 2);
            assert_eq!(session.decode(rect, &first[..cut]), Ok(None));
            assert_eq!(session.fb, Framebuffer::new(2, 2));
            // The stream is untouched: the full sequence still decodes.
            assert_eq!(session.decode(rect, &first), Ok(Some(first.len())));
            assert_eq!(session.decode(rect, &second), Ok(Some(second.len())));
            assert_eq!(session.fb.get(1, 1), Some(0x0003_0201));
        }
    }

    #[test]
    fn invalid_tiles_are_rejected() {
        let rect = Rect::new(0, 0, 2, 2);
        for raw in [
            vec![17],                                                          // unused subencoding
            vec![129],                                                         // unused subencoding
            [vec![128], cp(RED).to_vec(), vec![4]].concat(),                   // run too long
            [vec![130], cp(RED).to_vec(), cp(RED).to_vec(), vec![2]].concat(), // index ≥ size
            [
                vec![3],
                cp(RED).to_vec(),
                cp(RED).to_vec(),
                cp(RED).to_vec(),
                vec![0xc0, 0],
            ]
            .concat(), // packed index 3 of 3
            vec![1, 0, 0],                                                     // short
            [vec![1], cp(RED).to_vec(), vec![0]].concat(),                     // trailing byte
        ] {
            let payload = wrap(&Deflater::default().chunk(&raw));
            assert!(
                Session::new(2, 2).decode(rect, &payload).is_err(),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn zlib_bombs_and_oversized_lengths_are_rejected() {
        let rect = Rect::new(0, 0, 2, 2);
        let mut raw = vec![1];
        raw.extend(cp(RED));
        raw.extend(vec![0u8; 100_000]);
        let payload = wrap(&Deflater::default().chunk(&raw));
        assert!(matches!(
            Session::new(2, 2).decode(rect, &payload),
            Err(DecodeError::LimitExceeded {
                what: "inflated data",
                ..
            })
        ));
        // A huge announced length is rejected before waiting for the data.
        assert!(matches!(
            Session::new(2, 2).decode(rect, &[0xff, 0xff, 0xff, 0xff]),
            Err(DecodeError::LimitExceeded {
                what: "ZRLE data",
                ..
            })
        ));
        assert_eq!(
            Session::new(2, 2).decode(Rect::new(1, 1, 2, 2), &[]),
            Err(DecodeError::RectOutOfBounds)
        );
    }

    #[test]
    fn non_compact_formats_use_full_pixels() {
        let format = PixelFormat {
            bits_per_pixel: 16,
            depth: 16,
            big_endian: true,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..PixelFormat::RGB888
        };
        let mut session = Session::new(1, 1);
        session.converter = PixelConverter::new(format);
        let payload = wrap(&Deflater::default().chunk(&[1, 0x00, 0x1f]));
        assert_eq!(
            session.decode(Rect::new(0, 0, 1, 1), &payload),
            Ok(Some(payload.len()))
        );
        assert_eq!(session.fb.get(0, 0), Some(BLUE));
    }

    proptest! {
        #[test]
        fn decode_never_panics(
            raw in proptest::collection::vec(any::<u8>(), 0..512),
            w in 0u16..80, h in 0u16..10,
        ) {
            let payload = wrap(&Deflater::default().chunk(&raw));
            let mut session = Session::new(80, 10);
            let _ = session.decode(Rect::new(0, 0, w, h), &payload);
        }

        #[test]
        fn raw_garbage_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = Session::new(8, 8).decode(Rect::new(0, 0, 8, 8), &bytes);
        }
    }
}
