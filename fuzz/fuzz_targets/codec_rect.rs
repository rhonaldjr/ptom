#![no_main]
//! Arbitrary rectangle headers and payloads into the viewer's decoder.

use farsight_codec::Decoder;
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Limits, PixelFormat, RectHeader};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut decoder = Decoder::new(64, 48, PixelFormat::RGB888, Limits::DEFAULT);
    let mut rest = data;
    while rest.len() >= 12 {
        let Ok(header) = RectHeader::decode(rest) else { return };
        rest = &rest[12..];
        match decoder.decode_rect(&header, rest) {
            Ok(Some(used)) => {
                assert!(used <= rest.len());
                rest = &rest[used..];
            }
            Ok(None) | Err(_) => return,
        }
    }
});
