#![no_main]
//! Arbitrary rectangle headers and payloads into the viewer's decoder, in a
//! fuzzer-chosen pixel format.

use farsight_codec::Decoder;
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Limits, PixelFormat, RectHeader};
use libfuzzer_sys::fuzz_target;

fn format(selector: u8) -> PixelFormat {
    let candidate = PixelFormat {
        bits_per_pixel: [8, 16, 32][usize::from(selector % 3)],
        depth: [8, 16, 24][usize::from(selector % 3)],
        big_endian: selector & 0x08 != 0,
        true_colour: selector & 0x10 == 0,
        red_max: if selector % 3 == 1 { 31 } else { 255 },
        green_max: if selector % 3 == 1 { 63 } else { 255 },
        blue_max: if selector % 3 == 1 { 31 } else { 255 },
        red_shift: if selector % 3 == 1 { 11 } else { 16 },
        green_shift: if selector % 3 == 1 { 5 } else { 8 },
        blue_shift: 0,
    };
    if candidate.validate().is_ok() {
        candidate
    } else {
        PixelFormat::RGB888
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, mut rest)) = data.split_first() else {
        return;
    };
    let mut decoder = Decoder::new(64, 48, format(selector), Limits::DEFAULT);
    while rest.len() >= 12 {
        let Ok(header) = RectHeader::decode(rest) else { return };
        rest = &rest[12..];
        let before = decoder.framebuffer().clone();
        match decoder.decode_rect(&header, rest) {
            Ok(Some(used)) => {
                assert!(used <= rest.len());
                rest = &rest[used..];
            }
            Ok(None) => {
                assert_eq!(decoder.framebuffer(), &before);
                return;
            }
            Err(_) => return,
        }
    }
});
