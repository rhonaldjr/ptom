#![no_main]
//! Open H.264 rectangles (encoding 50) into the viewer's decoder. The H.264
//! decoding itself is Cisco's; this fuzzes Farsight's part: the payload
//! framing, context flags and limits, and pictures of the wrong size. A
//! stand-in decoder returns a picture whose size comes from the data.

use std::sync::Arc;

use farsight_codec::Decoder;
use farsight_codec::video::{Picture, VideoCodecs, VideoDecoder, VideoEncoder, VideoError};
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Limits, PixelFormat, RectHeader};
use libfuzzer_sys::fuzz_target;

struct FakeCodecs;
struct FakeDecoder;

impl VideoDecoder for FakeDecoder {
    fn decode(&mut self, data: &[u8], _size: (u16, u16)) -> Result<Option<Picture>, VideoError> {
        match data {
            [] | [0, ..] => Ok(None),
            [1, ..] => Err(VideoError("rejected".into())),
            [_, w, h, rest @ ..] => {
                let (width, height) = (u16::from(*w), u16::from(*h));
                // Sometimes fewer pixels than the size claims.
                let count = (usize::from(width) * usize::from(height)).min(rest.len() * 64);
                Ok(Some(Picture { width, height, pixels: vec![0x00ff_0000; count] }))
            }
            _ => Ok(None),
        }
    }
}

impl VideoCodecs for FakeCodecs {
    fn decoder(&self) -> Result<Box<dyn VideoDecoder>, VideoError> {
        Ok(Box::new(FakeDecoder))
    }
    fn encoder(&self, _: u16, _: u16) -> Result<Box<dyn VideoEncoder>, VideoError> {
        Err(VideoError("viewer only".into()))
    }
}

fuzz_target!(|data: &[u8]| {
    let mut decoder = Decoder::new(64, 48, PixelFormat::RGB888, Limits::DEFAULT);
    decoder.set_video(Arc::new(FakeCodecs));
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
