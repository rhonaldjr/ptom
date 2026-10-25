//! Q4 hardware check: frames encoded on this computer's GPU through VA-API
//! decode (with Cisco's OpenH264, as the viewer does) to nearly the same
//! pixels. Needs a GPU whose VA-API driver encodes H.264 and network access
//! for OpenH264; skipped, with a message, without them (CI has no GPU).
//! Run on a GPU machine with `cargo test -p farsight-hwenc --features vaapi`.

#![cfg(feature = "vaapi")]
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use farsight_codec::video::VideoCodecs;
use farsight_h264::{H264Error, OpenH264, install};
use farsight_hwenc::{Gpu, HardwareCodecs};

fn software() -> Option<OpenH264> {
    let dir = std::env::temp_dir().join("farsight-hwenc-test-openh264");
    std::fs::create_dir_all(&dir).unwrap();
    match install(&dir) {
        Ok(_) => Some(OpenH264::load(&dir).unwrap()),
        Err(H264Error::Download(reason)) => {
            eprintln!("skipping: can't download OpenH264 ({reason})");
            None
        }
        Err(other) => panic!("{other}"),
    }
}

/// Checkerboard scrolled by `shift` pixels.
fn frame(w: u16, h: u16, shift: usize) -> Vec<u32> {
    (0..usize::from(w) * usize::from(h))
        .map(|i| {
            let (x, y) = (i % usize::from(w) + shift, i / usize::from(w));
            if (x / 32 + y / 32) % 2 == 0 {
                0x00d0_3030
            } else {
                0x0030_60d0
            }
        })
        .collect()
}

fn distance(a: u32, b: u32) -> u32 {
    let [_, ar, ag, ab] = a.to_be_bytes();
    let [_, br, bg, bb] = b.to_be_bytes();
    u32::from(ar.abs_diff(br)) + u32::from(ag.abs_diff(bg)) + u32::from(ab.abs_diff(bb))
}

#[test]
fn gpu_encoded_frames_decode_close_to_the_original() {
    if farsight_hwenc::detect() != Some(Gpu::VaApi) {
        eprintln!("skipping: no VA-API H.264 encoder on this computer");
        return;
    }
    let Some(openh264) = software() else { return };
    // 360 isn't a multiple of 16: the stream must crop the padding away.
    let (w, h) = (640, 360);
    let codecs = HardwareCodecs::new(Gpu::VaApi, None);
    let mut encoder = codecs.encoder(w, h).unwrap();
    let mut decoder = openh264.decoder().unwrap();
    let mut errors = Vec::new();
    for shift in 0..30 {
        // Even steps: an odd shift puts colour edges inside 2×2 chroma
        // blocks, which 4:2:0 blurs whatever the encoder does.
        let pixels = frame(w, h, shift * 4);
        let stream = encoder.encode(&pixels).unwrap();
        assert!(!stream.is_empty(), "frame {shift} produced no data");
        if let Some(dump) = std::env::var_os("FARSIGHT_DUMP_H264") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dump)
                .unwrap();
            file.write_all(&stream).unwrap();
        }
        let picture = decoder
            .decode(&stream, (w, h))
            .unwrap()
            .unwrap_or_else(|| panic!("frame {shift} didn't decode"));
        assert_eq!((picture.width, picture.height), (w, h));
        let total: u64 = pixels
            .iter()
            .zip(&picture.pixels)
            .map(|(&a, &b)| u64::from(distance(a, b)))
            .sum();
        let average = total / pixels.len() as u64;
        errors.push(average);
    }
    let worst = errors.iter().max().copied().unwrap_or(0);
    assert!(worst < 6, "average colour errors per frame: {errors:?}");
}
