#![no_main]
//! A sequence of ZRLE, Tight and Hextile rectangles through one decoder, so
//! the zlib streams (and their reset bits) persist across rectangles.
//!
//! The fuzzer chooses the pixel format, and for each rectangle whether its
//! data is fed as-is or first compressed with a persistent zlib stream (so
//! the tile and filter parsers behind inflation are reached). Every
//! rectangle is also fed one byte short first: that must wait (`Ok(None)`)
//! or fail, and must not touch the framebuffer or the cursor.

use farsight_codec::Decoder;
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Encoding, Limits, PixelFormat, Rect, RectHeader};
use flate2::{Compress, Compression, FlushCompress};
use libfuzzer_sys::fuzz_target;

const RGB565: PixelFormat = PixelFormat {
    bits_per_pixel: 16,
    depth: 16,
    big_endian: false,
    true_colour: true,
    red_max: 31,
    green_max: 63,
    blue_max: 31,
    red_shift: 11,
    green_shift: 5,
    blue_shift: 0,
};

fn format(selector: u8) -> PixelFormat {
    match selector % 6 {
        0 => PixelFormat::RGB888,
        1 => PixelFormat {
            big_endian: true,
            ..PixelFormat::RGB888
        },
        2 => PixelFormat {
            red_shift: 24,
            green_shift: 16,
            blue_shift: 8,
            ..PixelFormat::RGB888
        },
        3 => RGB565,
        4 => PixelFormat {
            bits_per_pixel: 8,
            depth: 8,
            true_colour: false,
            ..PixelFormat::RGB888
        },
        _ => PixelFormat {
            depth: 32,
            ..PixelFormat::RGB888
        },
    }
}

struct Deflater(Compress);

impl Deflater {
    fn new() -> Self {
        Self(Compress::new(Compression::fast(), true))
    }

    fn chunk(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() * 2 + 64);
        let start = self.0.total_in();
        loop {
            let done = (self.0.total_in() - start) as usize;
            if self
                .0
                .compress_vec(&data[done..], &mut out, FlushCompress::Sync)
                .is_err()
            {
                return out;
            }
            let done = (self.0.total_in() - start) as usize;
            if done == data.len() && out.len() < out.capacity() {
                return out;
            }
            out.reserve(out.capacity().max(64));
        }
    }
}

fn compact(len: usize) -> Vec<u8> {
    let len = len.min(0x3f_ffff);
    let mut out = vec![(len & 0x7f) as u8];
    if len > 0x7f {
        out[0] |= 0x80;
        out.push(((len >> 7) & 0x7f) as u8);
        if len > 0x3fff {
            out[1] |= 0x80;
            out.push((len >> 14) as u8);
        }
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, mut rest)) = data.split_first() else {
        return;
    };
    let mut decoder = Decoder::new(80, 70, format(selector), Limits::DEFAULT);
    let mut zrle = Deflater::new();
    let mut tight = [Deflater::new(), Deflater::new(), Deflater::new(), Deflater::new()];
    while rest.len() >= 7 {
        let (head, tail) = rest.split_at(7);
        let op = head[0];
        let rect = Rect::new(
            u16::from(head[1] % 90),
            u16::from(head[2] % 80),
            u16::from(head[3] % 90),
            u16::from(head[4] % 80),
        );
        let len = usize::from(u16::from_be_bytes([head[5], head[6]])).min(tail.len());
        let (body, tail) = tail.split_at(len);
        rest = tail;
        let compress = op & 0x04 != 0;
        let (encoding, payload) = match op & 0x03 {
            0 | 1 if compress => {
                let chunk = zrle.chunk(body);
                let mut payload = (chunk.len() as u32).to_be_bytes().to_vec();
                payload.extend(chunk);
                (Encoding::Zrle, payload)
            }
            0 | 1 => (Encoding::Zrle, body.to_vec()),
            2 if compress && !body.is_empty() => {
                // body = control, prefix length, prefix (filter header), data.
                let control = body[0] & 0x7f;
                let prefix_len = usize::from(body.get(1).copied().unwrap_or(0) % 16);
                let after = body.get(2..).unwrap_or_default();
                let (prefix, raw) = after.split_at(prefix_len.min(after.len()));
                let chunk = tight[usize::from((control >> 4) & 3)].chunk(raw);
                let mut payload = vec![control];
                payload.extend_from_slice(prefix);
                payload.extend(compact(chunk.len()));
                payload.extend(chunk);
                (Encoding::Tight, payload)
            }
            2 => (Encoding::Tight, body.to_vec()),
            _ => (Encoding::Hextile, body.to_vec()),
        };
        let header = RectHeader { rect, encoding };
        if let Some(short) = payload.len().checked_sub(1) {
            let before = decoder.framebuffer().clone();
            let generation = decoder.cursor_generation();
            if let Ok(None) = decoder.decode_rect(&header, &payload[..short]) {
                assert_eq!(decoder.framebuffer(), &before);
                assert_eq!(decoder.cursor_generation(), generation);
            } else {
                // The truncated payload was decoded or rejected; the stream
                // states no longer match the server's, so stop here.
                return;
            }
        }
        match decoder.decode_rect(&header, &payload) {
            Ok(Some(used)) => assert!(used <= payload.len()),
            Ok(None) | Err(_) => return,
        }
    }
});
