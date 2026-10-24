//! Tight (encoding 7): fill, JPEG, and basic compression with the copy,
//! palette and gradient filters over four persistent zlib streams
//! (rfbproto "Tight Encoding"). `TightPNG` is a different encoding number and
//! is not supported.
//!
//! The whole rectangle is parsed (and lengths checked) before anything is
//! touched: stream resets and inflation happen only once every byte is here.

use std::borrow::Cow;

use farsight_rfb::wire::Reader;
use farsight_rfb::{DecodeError, Rect};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use crate::compact::CompactPixel;
use crate::inflate::ZlibStream;
use crate::util::{compact_length, mul, need};
use crate::{Framebuffer, PixelConverter};

/// Tight rectangles may not be wider than this.
pub(crate) const MAX_WIDTH: u16 = 2048;
/// Filtered data shorter than this is sent without zlib.
const MIN_TO_COMPRESS: usize = 12;
/// Number of zlib streams.
pub(crate) const STREAMS: usize = 4;

const FILTER_COPY: u8 = 0;
const FILTER_PALETTE: u8 = 1;
const FILTER_GRADIENT: u8 = 2;

enum Filter {
    Copy,
    Palette(Vec<u32>),
    Gradient,
}

enum Data<'a> {
    Plain(&'a [u8]),
    Zlib { stream: usize, bytes: &'a [u8] },
}

enum Plan<'a> {
    Fill(u32),
    Jpeg(&'a [u8]),
    Basic {
        filter: Filter,
        len: usize,
        data: Data<'a>,
    },
}

/// Decodes a Tight payload into `fb`.
pub(crate) fn decode(
    fb: &mut Framebuffer,
    streams: &mut [ZlibStream; STREAMS],
    converter: &PixelConverter,
    rect: Rect,
    input: &[u8],
) -> Result<Option<usize>, DecodeError> {
    fb.check(rect)?;
    if rect.width > MAX_WIDTH {
        return Err(DecodeError::LimitExceeded {
            what: "Tight rectangle width",
            len: u64::from(rect.width),
            max: u64::from(MAX_WIDTH),
        });
    }
    let tpixel = CompactPixel::tpixel(converter);
    let mut r = Reader::new(input);
    let control = need!(r.u8());
    let plan = match control >> 4 {
        0x8 => Plan::Fill(tpixel.rgb(need!(r.bytes(tpixel.len())))),
        0x9 => {
            let len = need!(compact_length(&mut r));
            Plan::Jpeg(need!(r.bytes(len)))
        }
        kind @ 0x0..=0x7 => {
            let filter = if kind & 0x4 == 0 {
                Filter::Copy
            } else {
                need!(read_filter(&mut r, tpixel))
            };
            let len = filtered_len(&filter, rect, tpixel)?;
            let data = if len < MIN_TO_COMPRESS {
                Data::Plain(need!(r.bytes(len)))
            } else {
                let compressed = need!(compact_length(&mut r));
                Data::Zlib {
                    stream: usize::from(kind & 0x3),
                    bytes: need!(r.bytes(compressed)),
                }
            };
            Plan::Basic { filter, len, data }
        }
        kind @ (0xA | 0xE) => {
            let filter = if kind == 0xE {
                need!(read_filter(&mut r, tpixel))
            } else {
                Filter::Copy
            };
            let len = filtered_len(&filter, rect, tpixel)?;
            let data = Data::Plain(need!(r.bytes(len)));
            Plan::Basic { filter, len, data }
        }
        _ => return Err(DecodeError::Malformed("invalid Tight compression control")),
    };
    // The whole rectangle is present: from here on, state may change.
    for (bit, stream) in streams.iter_mut().enumerate() {
        if control & (1 << bit) != 0 {
            stream.reset();
        }
    }
    match plan {
        Plan::Fill(rgb) => fb.fill(rect, rgb)?,
        Plan::Jpeg(bytes) => jpeg(fb, rect, bytes)?,
        Plan::Basic { filter, len, data } => {
            let bytes = match data {
                Data::Plain(bytes) => Cow::Borrowed(bytes),
                Data::Zlib { stream, bytes } => Cow::Owned(
                    streams
                        .get_mut(stream)
                        .ok_or(DecodeError::Malformed("Tight stream out of range"))?
                        .inflate_exact(bytes, len)?,
                ),
            };
            apply_filter(fb, rect, tpixel, &filter, &bytes)?;
        }
    }
    Ok(Some(r.consumed()))
}

fn read_filter(r: &mut Reader<'_>, tpixel: CompactPixel<'_>) -> Result<Filter, DecodeError> {
    match r.u8()? {
        FILTER_COPY => Ok(Filter::Copy),
        FILTER_PALETTE => {
            let colours = usize::from(r.u8()?) + 1;
            let bytes = r.bytes(colours * tpixel.len())?;
            Ok(Filter::Palette(
                bytes
                    .chunks_exact(tpixel.len())
                    .map(|p| tpixel.rgb(p))
                    .collect(),
            ))
        }
        FILTER_GRADIENT => {
            let f = tpixel.converter().format();
            if f.true_colour && matches!(f.bits_per_pixel, 16 | 32) {
                Ok(Filter::Gradient)
            } else {
                Err(DecodeError::Malformed(
                    "Tight gradient filter needs a 16 or 32 bpp true-colour format",
                ))
            }
        }
        _ => Err(DecodeError::Malformed("unknown Tight filter")),
    }
}

/// Size of the pixel data after filtering, before compression.
fn filtered_len(
    filter: &Filter,
    rect: Rect,
    tpixel: CompactPixel<'_>,
) -> Result<usize, DecodeError> {
    match filter {
        Filter::Copy | Filter::Gradient => mul(rect.area(), tpixel.len()),
        Filter::Palette(colours) if colours.len() == 2 => mul(
            usize::from(rect.width).div_ceil(8),
            usize::from(rect.height),
        ),
        Filter::Palette(_) => Ok(rect.area()),
    }
}

fn apply_filter(
    fb: &mut Framebuffer,
    rect: Rect,
    tpixel: CompactPixel<'_>,
    filter: &Filter,
    bytes: &[u8],
) -> Result<(), DecodeError> {
    let len = tpixel.len();
    match filter {
        Filter::Copy => fb.write(rect, |i| {
            tpixel.rgb(bytes.get(i * len..).unwrap_or_default())
        }),
        Filter::Palette(colours) if colours.len() == 2 => {
            let width = usize::from(rect.width).max(1);
            let row_bytes = width.div_ceil(8);
            fb.write(rect, |i| {
                let (y, x) = (i / width, i % width);
                let byte = bytes.get(y * row_bytes + x / 8).copied().unwrap_or(0);
                let index = usize::from((byte >> (7 - x % 8)) & 1);
                colours.get(index).copied().unwrap_or(0)
            })
        }
        Filter::Palette(colours) => {
            if bytes
                .iter()
                .any(|&index| usize::from(index) >= colours.len())
            {
                return Err(DecodeError::Malformed("Tight palette index out of range"));
            }
            fb.write(rect, |i| {
                bytes
                    .get(i)
                    .and_then(|&index| colours.get(usize::from(index)))
                    .copied()
                    .unwrap_or(0)
            })
        }
        Filter::Gradient => {
            let pixels = gradient(rect, tpixel, bytes);
            fb.write(rect, |i| pixels.get(i).copied().unwrap_or(0))
        }
    }
}

/// Undoes the gradient filter: each component is the difference from
/// `left + above - above_left`, clamped to `0..=max`, modulo `max + 1`.
fn gradient(rect: Rect, tpixel: CompactPixel<'_>, bytes: &[u8]) -> Vec<u32> {
    let width = usize::from(rect.width);
    let len = tpixel.len();
    let f = tpixel.converter().format();
    let channels: [(u32, u32); 3] = if tpixel.is_rgb() {
        [(255, 16), (255, 8), (255, 0)]
    } else {
        [
            (u32::from(f.red_max), u32::from(f.red_shift)),
            (u32::from(f.green_max), u32::from(f.green_shift)),
            (u32::from(f.blue_max), u32::from(f.blue_shift)),
        ]
    };
    let components = |pixel: &[u8]| -> [u32; 3] {
        let raw = if tpixel.is_rgb() {
            let mut value = 0u32;
            for &b in pixel {
                value = (value << 8) | u32::from(b);
            }
            value
        } else {
            tpixel.converter().read_raw(pixel)
        };
        channels.map(|(max, shift)| raw.checked_shr(shift).unwrap_or(0) & max)
    };
    let compose = |values: [u32; 3]| -> u32 {
        let raw = values
            .iter()
            .zip(channels)
            .fold(0u32, |acc, (&v, (_, shift))| {
                acc | v.checked_shl(shift).unwrap_or(0)
            });
        if tpixel.is_rgb() {
            raw
        } else {
            tpixel.converter().to_rgb(raw)
        }
    };
    let maxes = channels.map(|(max, _)| max);
    let mut out = Vec::with_capacity(rect.area());
    let mut above = vec![[0u32; 3]; width];
    if width == 0 || len == 0 {
        return out;
    }
    for row in bytes
        .chunks_exact(width * len)
        .take(usize::from(rect.height))
    {
        let mut left = [0u32; 3];
        let mut above_left = [0u32; 3];
        for (pixel, up) in row.chunks_exact(len).zip(above.iter_mut()) {
            let value = predict(left, *up, above_left, components(pixel), maxes);
            above_left = *up;
            *up = value;
            left = value;
            out.push(compose(value));
        }
    }
    out
}

fn predict(
    left: [u32; 3],
    above: [u32; 3],
    above_left: [u32; 3],
    diff: [u32; 3],
    max: [u32; 3],
) -> [u32; 3] {
    let one = |l: u32, a: u32, al: u32, d: u32, m: u32| -> u32 {
        let guess = (i64::from(l) + i64::from(a) - i64::from(al)).clamp(0, i64::from(m));
        let modulus = i64::from(m) + 1;
        u32::try_from((guess + i64::from(d)) % modulus).unwrap_or(0)
    };
    let [l0, l1, l2] = left;
    let [a0, a1, a2] = above;
    let [al0, al1, al2] = above_left;
    let [d0, d1, d2] = diff;
    let [m0, m1, m2] = max;
    [
        one(l0, a0, al0, d0, m0),
        one(l1, a1, al1, d1, m1),
        one(l2, a2, al2, d2, m2),
    ]
}

fn jpeg(fb: &mut Framebuffer, rect: Rect, data: &[u8]) -> Result<(), DecodeError> {
    let bad = |_| DecodeError::Malformed("invalid JPEG data");
    if rect.is_empty() {
        return Ok(());
    }
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(usize::from(rect.width))
        .set_max_height(usize::from(rect.height));
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    decoder.decode_headers().map_err(bad)?;
    let info = decoder
        .info()
        .ok_or(DecodeError::Malformed("invalid JPEG data"))?;
    if (info.width, info.height) != (rect.width, rect.height) {
        return Err(DecodeError::Malformed(
            "JPEG size differs from the rectangle",
        ));
    }
    let size = decoder
        .output_buffer_size()
        .ok_or(DecodeError::Malformed("invalid JPEG data"))?;
    let components = size / rect.area();
    if !matches!(components, 1 | 3) || size != components * rect.area() {
        return Err(DecodeError::Malformed("unexpected JPEG colour space"));
    }
    let mut out = vec![0u8; size];
    decoder.decode_into(&mut out).map_err(bad)?;
    fb.write(rect, |i| {
        let px = out.get(i * components..).unwrap_or_default();
        match *px {
            [r, g, b, ..] if components == 3 => {
                (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
            }
            [grey, ..] => u32::from(grey) * 0x0001_0101,
            [] => 0,
        }
    })
}

#[cfg(test)]
pub(crate) mod test_support {
    /// Builds a baseline greyscale JPEG of one 8×8 block (cropped to
    /// `width` × `height` ≤ 8) whose every pixel is `level`. Test data only:
    /// one quantisation table of ones, single-code Huffman tables for the DC
    /// category and the end-of-block AC symbol.
    pub(crate) fn solid_grey_jpeg(width: u8, height: u8, level: u8) -> Vec<u8> {
        let dc = (i32::from(level) - 128) * 8;
        let category = u8::try_from(32 - dc.unsigned_abs().leading_zeros()).unwrap();
        let mut j = vec![0xff, 0xd8];
        // DQT: table 0, 8-bit, all ones.
        j.extend([0xff, 0xdb, 0x00, 0x43, 0x00]);
        j.extend([1u8; 64]);
        // SOF0: 8-bit, height, width, 1 component (id 1, 1×1, table 0).
        j.extend([
            0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, height, 0x00, width, 0x01, 0x01, 0x11, 0x00,
        ]);
        // DHT DC 0: one code of length 1 for `category`.
        j.extend([0xff, 0xc4, 0x00, 0x14, 0x00, 1]);
        j.extend([0u8; 15]);
        j.push(category);
        // DHT AC 0: one code of length 1 for EOB (0x00).
        j.extend([0xff, 0xc4, 0x00, 0x14, 0x10, 1]);
        j.extend([0u8; 15]);
        j.push(0x00);
        // SOS: 1 component, tables 0/0, full spectral range.
        j.extend([0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]);
        // Entropy data: DC code "0", `category` magnitude bits, AC EOB "0".
        let magnitude = if dc >= 0 {
            u32::try_from(dc).unwrap()
        } else {
            u32::try_from(dc + (1 << category) - 1).unwrap()
        };
        let mut bits: Vec<bool> = vec![false];
        bits.extend((0..category).rev().map(|i| (magnitude >> i) & 1 == 1));
        bits.push(false);
        while !bits.len().is_multiple_of(8) {
            bits.push(true);
        }
        for byte in bits.chunks(8) {
            let value = byte.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b));
            j.push(value);
            if value == 0xff {
                j.push(0);
            }
        }
        j.extend([0xff, 0xd9]);
        j
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::solid_grey_jpeg;
    use super::*;
    use crate::inflate::test_support::Deflater;
    use farsight_rfb::PixelFormat;
    use proptest::prelude::*;

    const RED: u32 = 0x00ff_0000;
    const GREEN: u32 = 0x0000_ff00;
    const BLUE: u32 = 0x0000_00ff;

    fn tp(rgb: u32) -> [u8; 3] {
        let [b, g, r, _] = rgb.to_le_bytes();
        [r, g, b]
    }

    fn compact(len: usize) -> Vec<u8> {
        let mut out = vec![u8::try_from(len & 0x7f).unwrap()];
        if len > 0x7f {
            out[0] |= 0x80;
            out.push(u8::try_from((len >> 7) & 0x7f).unwrap());
            if len > 0x3fff {
                out[1] |= 0x80;
                out.push(u8::try_from(len >> 14).unwrap());
            }
        }
        out
    }

    struct Session {
        fb: Framebuffer,
        streams: [ZlibStream; STREAMS],
        converter: PixelConverter,
    }

    impl Session {
        fn new(w: u16, h: u16) -> Self {
            Self {
                fb: Framebuffer::new(w, h),
                streams: Default::default(),
                converter: PixelConverter::new(PixelFormat::RGB888),
            }
        }
        fn decode(&mut self, rect: Rect, input: &[u8]) -> Result<Option<usize>, DecodeError> {
            decode(
                &mut self.fb,
                &mut self.streams,
                &self.converter,
                rect,
                input,
            )
        }
        fn ok(&mut self, rect: Rect, input: &[u8]) {
            assert_eq!(self.decode(rect, input), Ok(Some(input.len())));
        }
    }

    /// Basic compression: `control` then optional filter header then the data,
    /// zlib-compressed with `deflater` when it is 12 bytes or more.
    fn basic(control: u8, header: &[u8], data: &[u8], deflater: &mut Deflater) -> Vec<u8> {
        let mut out = vec![control];
        out.extend_from_slice(header);
        if data.len() < MIN_TO_COMPRESS {
            out.extend_from_slice(data);
        } else {
            let chunk = deflater.chunk(data);
            out.extend(compact(chunk.len()));
            out.extend(chunk);
        }
        out
    }

    #[test]
    fn fill_paints_the_rectangle() {
        let mut s = Session::new(4, 4);
        let payload = [vec![0x80], tp(GREEN).to_vec()].concat();
        s.ok(Rect::new(1, 1, 2, 2), &payload);
        assert_eq!(s.fb.get(1, 1), Some(GREEN));
        assert_eq!(s.fb.get(2, 2), Some(GREEN));
        assert_eq!(s.fb.get(0, 0), Some(0));
    }

    #[test]
    fn copy_filter_small_data_is_sent_uncompressed() {
        let mut s = Session::new(3, 1);
        let data: Vec<u8> = [RED, GREEN, BLUE].iter().flat_map(|&c| tp(c)).collect();
        assert_eq!(data.len(), 9);
        let payload = basic(0x00, &[], &data, &mut Deflater::default());
        assert_eq!(payload.len(), 10);
        s.ok(Rect::new(0, 0, 3, 1), &payload);
        assert_eq!(s.fb.pixels(), [RED, GREEN, BLUE]);
    }

    #[test]
    fn copy_filter_uses_the_selected_stream_across_rects() {
        let mut s = Session::new(4, 2);
        let rect = Rect::new(0, 0, 4, 2);
        let data: Vec<u8> = [RED, GREEN, BLUE, RED, GREEN, BLUE, RED, GREEN]
            .iter()
            .flat_map(|&c| tp(c))
            .collect();
        let mut streams: [Deflater; 4] = Default::default();
        for round in 0..2 {
            for id in 0..4u8 {
                let control = (id << 4) | if id == 2 { 0x40 } else { 0 };
                let header: &[u8] = if id == 2 { &[FILTER_COPY] } else { &[] };
                let payload = basic(control, header, &data, &mut streams[usize::from(id)]);
                s.ok(rect, &payload);
                assert_eq!(s.fb.get(3, 1), Some(GREEN), "round {round} stream {id}");
            }
        }
    }

    #[test]
    fn reset_bits_restart_streams_even_for_fill() {
        let mut s = Session::new(4, 4);
        let rect = Rect::new(0, 0, 4, 4);
        let data = [tp(RED); 16].concat();
        let mut deflater = Deflater::default();
        s.ok(rect, &basic(0x10, &[], &data, &mut deflater));
        // Server resets stream 1 while sending a fill, then starts afresh.
        s.ok(rect, &[vec![0x82], tp(BLUE).to_vec()].concat());
        let mut fresh = Deflater::default();
        s.ok(rect, &basic(0x10, &[], &data, &mut fresh));
        assert_eq!(s.fb.get(3, 3), Some(RED));
        // Without the reset, a fresh stream's header would be corrupt.
        let mut s2 = Session::new(4, 4);
        s2.ok(rect, &basic(0x10, &[], &data, &mut Deflater::default()));
        assert!(
            s2.decode(rect, &basic(0x10, &[], &data, &mut Deflater::default()))
                .is_err()
        );
    }

    #[test]
    fn two_colour_palette_packs_one_bit_per_pixel_with_row_padding() {
        // 10×2: rows 1010000001, 0101111110.
        let mut s = Session::new(10, 2);
        let header = [vec![FILTER_PALETTE, 1], tp(RED).to_vec(), tp(BLUE).to_vec()].concat();
        let data = [0b1010_0000, 0b0100_0000, 0b0101_1111, 0b1000_0000];
        s.ok(
            Rect::new(0, 0, 10, 2),
            &basic(0x40, &header, &data, &mut Deflater::default()),
        );
        let row0: Vec<u32> = "1010000001"
            .chars()
            .map(|c| if c == '1' { BLUE } else { RED })
            .collect();
        let row1: Vec<u32> = "0101111110"
            .chars()
            .map(|c| if c == '1' { BLUE } else { RED })
            .collect();
        assert_eq!(s.fb.pixels(), [row0, row1].concat());
    }

    #[test]
    fn larger_palettes_use_a_byte_per_pixel() {
        let mut s = Session::new(4, 4);
        let header = [
            vec![FILTER_PALETTE, 2],
            tp(RED).to_vec(),
            tp(GREEN).to_vec(),
            tp(BLUE).to_vec(),
        ]
        .concat();
        let data: Vec<u8> = (0..16).map(|i| i % 3).collect();
        s.ok(
            Rect::new(0, 0, 4, 4),
            &basic(0x40, &header, &data, &mut Deflater::default()),
        );
        assert_eq!(s.fb.get(0, 0), Some(RED));
        assert_eq!(s.fb.get(1, 0), Some(GREEN));
        assert_eq!(s.fb.get(2, 0), Some(BLUE));
        assert_eq!(s.fb.get(3, 3), Some(RED));
        let bad: Vec<u8> = vec![3; 16];
        assert!(matches!(
            s.decode(
                Rect::new(0, 0, 4, 4),
                &basic(0x40, &header, &bad, &mut Deflater::default())
            ),
            Err(DecodeError::Malformed(_))
        ));
    }

    /// Applies the gradient filter (test-only encoder) to RGB pixels.
    fn gradient_encode(pixels: &[u32], width: usize) -> Vec<u8> {
        let comp = |p: u32| [(p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff].map(i64::from);
        let at = |x: Option<usize>, y: Option<usize>| match (x, y) {
            (Some(x), Some(y)) => comp(pixels[y * width + x]),
            _ => [0; 3],
        };
        let mut out = Vec::new();
        for (i, &p) in pixels.iter().enumerate() {
            let (x, y) = (i % width, i / width);
            let left = at(x.checked_sub(1), Some(y));
            let above = at(Some(x), y.checked_sub(1));
            let above_left = at(x.checked_sub(1), y.checked_sub(1));
            let value = comp(p);
            for c in 0..3 {
                let guess = (left[c] + above[c] - above_left[c]).clamp(0, 255);
                out.push(u8::try_from((value[c] - guess).rem_euclid(256)).unwrap());
            }
        }
        out
    }

    #[test]
    fn gradient_filter_reconstructs_pixels() {
        let width = 5;
        let pixels: Vec<u32> = (0..15u32)
            .map(|i| ((i * 40 % 256) << 16) | ((255 - i * 17) << 8) | (i * i % 256))
            .collect();
        let data = gradient_encode(&pixels, width);
        let mut s = Session::new(5, 3);
        s.ok(
            Rect::new(0, 0, 5, 3),
            &basic(0x40, &[FILTER_GRADIENT], &data, &mut Deflater::default()),
        );
        assert_eq!(s.fb.pixels(), pixels.as_slice());
    }

    #[test]
    fn gradient_filter_with_16_bit_pixels() {
        let format = PixelFormat {
            bits_per_pixel: 16,
            depth: 16,
            big_endian: false,
            red_max: 31,
            green_max: 63,
            blue_max: 31,
            red_shift: 11,
            green_shift: 5,
            blue_shift: 0,
            ..PixelFormat::RGB888
        };
        // 2×1: first pixel (r=31,g=0,b=0) is its own difference; the second
        // (r=31,g=63,b=31) differs from the left prediction by (0,63,31).
        let d0: u16 = 31 << 11;
        let d1: u16 = (63 << 5) | 31;
        let data = [d0.to_le_bytes(), d1.to_le_bytes()].concat();
        let mut s = Session::new(2, 1);
        s.converter = PixelConverter::new(format);
        s.ok(
            Rect::new(0, 0, 2, 1),
            &basic(0x40, &[FILTER_GRADIENT], &data, &mut Deflater::default()),
        );
        assert_eq!(s.fb.pixels(), [RED, 0x00ff_ffff]);
    }

    #[test]
    fn without_zlib_variants_send_data_as_is() {
        let mut s = Session::new(4, 1);
        let data: Vec<u8> = [RED, GREEN, BLUE, RED]
            .iter()
            .flat_map(|&c| tp(c))
            .collect();
        s.ok(Rect::new(0, 0, 4, 1), &[vec![0xA0], data.clone()].concat());
        assert_eq!(s.fb.pixels(), [RED, GREEN, BLUE, RED]);
        let header = [
            vec![0xE0, FILTER_PALETTE, 1],
            tp(GREEN).to_vec(),
            tp(BLUE).to_vec(),
        ]
        .concat();
        s.ok(Rect::new(0, 0, 4, 1), &[header, vec![0b0110_0000]].concat());
        assert_eq!(s.fb.pixels(), [GREEN, BLUE, BLUE, GREEN]);
    }

    #[test]
    fn jpeg_rectangles_decode() {
        let jpeg = solid_grey_jpeg(4, 3, 200);
        let mut s = Session::new(8, 8);
        let payload = [vec![0x90], compact(jpeg.len()), jpeg.clone()].concat();
        s.ok(Rect::new(2, 2, 4, 3), &payload);
        let grey = s.fb.get(3, 3).unwrap();
        let [b, g, r, _] = grey.to_le_bytes();
        assert!((198..=202).contains(&r) && r == g && g == b, "{grey:06x}");
        assert_eq!(s.fb.get(0, 0), Some(0));
        // Size mismatch with the rectangle.
        let payload = [vec![0x90], compact(jpeg.len()), jpeg].concat();
        assert!(s.decode(Rect::new(0, 0, 5, 3), &payload).is_err());
        // Garbage.
        assert!(
            s.decode(Rect::new(0, 0, 1, 1), &[0x90, 3, 1, 2, 3])
                .is_err()
        );
    }

    #[test]
    fn truncation_at_every_cut_has_no_side_effects() {
        let rect = Rect::new(0, 0, 4, 4);
        let data = [tp(RED); 16].concat();
        let jpeg = solid_grey_jpeg(4, 4, 10);
        let payloads = [
            [vec![0x81], tp(BLUE).to_vec()].concat(),
            basic(0x1f, &[], &data, &mut Deflater::default()),
            basic(
                0x41,
                &[FILTER_PALETTE, 1, 1, 2, 3, 4, 5, 6],
                &[0xa0, 0x50, 0xa0, 0x50],
                &mut Deflater::default(),
            ),
            [vec![0x9f], compact(jpeg.len()), jpeg].concat(),
        ];
        for payload in payloads {
            for cut in 0..payload.len() {
                let mut s = Session::new(4, 4);
                // Prime stream 0 so a premature reset would be detectable.
                let mut deflater = Deflater::default();
                s.ok(rect, &basic(0x00, &[], &data, &mut deflater));
                let before = s.fb.clone();
                assert_eq!(s.decode(rect, &payload[..cut]), Ok(None), "cut {cut}");
                assert_eq!(s.fb, before);
                // Stream 0 still continues the primed stream.
                s.ok(rect, &basic(0x00, &[], &data, &mut deflater));
            }
        }
    }

    #[test]
    fn invalid_payloads_are_rejected() {
        let rect = Rect::new(0, 0, 4, 4);
        let mut s = Session::new(4, 4);
        // Invalid compression kinds.
        for control in [0xB0, 0xC0, 0xD0, 0xF0] {
            assert!(s.decode(rect, &[control, 0, 0, 0]).is_err());
        }
        // Unknown filter.
        assert!(s.decode(rect, &[0x40, 3]).is_err());
        // Zlib bomb: more output than the rectangle needs.
        let mut deflater = Deflater::default();
        let big = basic(0x00, &[], &[0; 4096], &mut deflater);
        assert!(matches!(
            s.decode(rect, &big),
            Err(DecodeError::LimitExceeded {
                what: "inflated data",
                ..
            })
        ));
        // Too little output.
        let short = basic(0x00, &[], &[0; 47], &mut Deflater::default());
        assert!(s.decode(rect, &short).is_err());
        // Too wide.
        let mut wide = Session::new(3000, 1);
        assert!(matches!(
            wide.decode(Rect::new(0, 0, 2049, 1), &[0x80, 0, 0, 0]),
            Err(DecodeError::LimitExceeded { .. })
        ));
        // Out of bounds.
        assert_eq!(
            s.decode(Rect::new(3, 3, 2, 2), &[]),
            Err(DecodeError::RectOutOfBounds)
        );
        // Gradient with an 8-bit colour-mapped format.
        s.converter = PixelConverter::new(PixelFormat {
            bits_per_pixel: 8,
            depth: 8,
            true_colour: false,
            ..PixelFormat::RGB888
        });
        assert!(s.decode(rect, &[0x40, FILTER_GRADIENT]).is_err());
    }

    proptest! {
        #[test]
        fn decode_never_panics(
            bytes in proptest::collection::vec(any::<u8>(), 0..256),
            w in 0u16..20, h in 0u16..20,
        ) {
            let mut s = Session::new(20, 20);
            let before = s.fb.clone();
            let result = s.decode(Rect::new(0, 0, w, h), &bytes);
            if let Ok(Some(used)) = result {
                prop_assert!(used <= bytes.len());
            } else if result == Ok(None) {
                prop_assert_eq!(s.fb, before);
            }
        }

        #[test]
        fn compressed_garbage_never_panics(
            control in any::<u8>(),
            filter in proptest::collection::vec(any::<u8>(), 0..8),
            data in proptest::collection::vec(any::<u8>(), 0..300),
            w in 0u16..12, h in 0u16..12,
        ) {
            let payload = basic(control, &filter, &data, &mut Deflater::default());
            let _ = Session::new(12, 12).decode(Rect::new(0, 0, w, h), &payload);
        }
    }
}
