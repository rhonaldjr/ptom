#![no_main]
//! Server-side encoders against the viewer's decoder: an arbitrary
//! framebuffer, pixel format and rectangle sequence must encode without
//! error and decode back to exactly what Raw would give, with the zlib
//! streams persisting (and Tight's being reset) across rectangles.

use farsight_codec::encode::{self, TightEncoder, ZrleEncoder};
use farsight_codec::{Decoder, Framebuffer, PixelConverter};
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Encoding, Limits, PixelFormat, Rect, RectHeader};
use libfuzzer_sys::fuzz_target;

fn format(selector: u8) -> PixelFormat {
    let rgb565 = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: selector & 8 != 0,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };
    match selector % 4 {
        0 => PixelFormat::RGB888,
        1 => rgb565,
        2 => PixelFormat {
            big_endian: true,
            red_shift: 24,
            green_shift: 16,
            blue_shift: 8,
            ..PixelFormat::RGB888
        },
        _ => PixelFormat {
            bits_per_pixel: 8,
            depth: 8,
            red_max: 7,
            green_max: 7,
            blue_max: 3,
            red_shift: 0,
            green_shift: 3,
            blue_shift: 6,
            ..PixelFormat::RGB888
        },
    }
}

fuzz_target!(|data: &[u8]| {
    let [w, h, fmt, level, rest @ ..] = data else {
        return;
    };
    let (w, h) = (u16::from(*w % 128) + 1, u16::from(*h % 128) + 1);
    let format = format(*fmt);
    let conv = PixelConverter::new(format);
    let (pixels, rects) = rest.split_at(rest.len() / 2);
    let mut fb = Framebuffer::new(w, h);
    fb.write(Rect::new(0, 0, w, h), |i| {
        let b = pixels.get(i % pixels.len().max(1)).copied().unwrap_or(0);
        (u32::from(b) * 0x0001_0307) & 0x00ff_ffff
    })
    .expect("fill");
    let mut zrle = ZrleEncoder::new(level % 10);
    let mut tight = TightEncoder::new(level % 10);
    let mut decoders = [Encoding::Raw, Encoding::Zrle, Encoding::Tight]
        .map(|e| (e, Decoder::new(w, h, format, Limits::DEFAULT)));
    let mut reference = Decoder::new(w, h, format, Limits::DEFAULT);
    for spec in rects.chunks_exact(4) {
        let x = u16::from(spec[0]) % w;
        let y = u16::from(spec[1]) % h;
        let rect = Rect::new(
            x,
            y,
            (u16::from(spec[2]) % (w - x)) + 1,
            (u16::from(spec[3]) % (h - y)) + 1,
        );
        let raw = encode::raw(&fb, rect, &conv).expect("raw");
        let header = RectHeader {
            rect,
            encoding: Encoding::Raw,
        };
        assert_eq!(reference.decode_rect(&header, &raw), Ok(Some(raw.len())));
        for (encoding, decoder) in &mut decoders {
            let payload = match encoding {
                Encoding::Zrle => zrle.encode(&fb, rect, &conv).expect("zrle"),
                Encoding::Tight => tight.encode(&fb, rect, &conv).expect("tight"),
                _ => raw.clone(),
            };
            let header = RectHeader {
                rect,
                encoding: *encoding,
            };
            assert_eq!(
                decoder.decode_rect(&header, &payload),
                Ok(Some(payload.len()))
            );
            assert_eq!(
                decoder.framebuffer().read(rect),
                reference.framebuffer().read(rect)
            );
        }
        if spec[0] & 1 == 1 {
            tight.set_compress_level(spec[1] % 10);
        }
    }
});
