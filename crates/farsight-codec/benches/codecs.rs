//! Q4 benchmarks (M8): encoding and decoding a full 1920×1080 frame, cut
//! into 256×256 pieces as the agent does, for each encoding, on two kinds of
//! content: a desktop (flat areas and text-like detail) and a photo.
//!
//! `cargo bench -p farsight-codec`; results are summarised in
//! docs/performance/baselines.md.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names
)]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use farsight_codec::encode::{self, TightEncoder, ZrleEncoder};
use farsight_codec::{Decoder, Framebuffer, PixelConverter};
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Encoding, Limits, PixelFormat, Rect, RectHeader};

const W: u16 = 1920;
const H: u16 = 1080;
const PIECE: u16 = 256;

fn pieces() -> Vec<Rect> {
    let mut out = Vec::new();
    for y in (0..H).step_by(usize::from(PIECE)) {
        for x in (0..W).step_by(usize::from(PIECE)) {
            out.push(Rect::new(x, y, PIECE.min(W - x), PIECE.min(H - y)));
        }
    }
    out
}

/// Windows, title bars and lines of "text" on a plain background.
fn desktop() -> Framebuffer {
    let mut fb = Framebuffer::new(W, H);
    fb.write(Rect::new(0, 0, W, H), |i| {
        let (x, y) = (i % usize::from(W), i / usize::from(W));
        let in_window = (200..1400).contains(&x) && (150..900).contains(&y);
        if !in_window {
            0x0020_3040
        } else if y < 180 {
            0x00e0_e0e0
        } else if (y / 18) % 2 == 0 && (x * 7 + y * 3) % 11 < 4 {
            0x0010_1010
        } else {
            0x00ff_ffff
        }
    })
    .unwrap();
    fb
}

/// A smooth, noisy picture: every pixel slightly different.
fn photo() -> Framebuffer {
    let mut fb = Framebuffer::new(W, H);
    fb.write(Rect::new(0, 0, W, H), |i| {
        let (x, y) = ((i % usize::from(W)) as u32, (i / usize::from(W)) as u32);
        let noise = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) & 0x0f;
        let r = (x * 255 / u32::from(W) + noise) & 0xff;
        let g = (y * 255 / u32::from(H) + noise) & 0xff;
        let b = ((x + y) / 12 + noise) & 0xff;
        r << 16 | g << 8 | b
    })
    .unwrap();
    fb
}

#[derive(Clone, Copy)]
enum Codec {
    Raw,
    Hextile,
    Zrle,
    Tight,
    TightJpeg,
}

impl Codec {
    const ALL: [Self; 5] = [
        Self::Raw,
        Self::Hextile,
        Self::Zrle,
        Self::Tight,
        Self::TightJpeg,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Hextile => "hextile",
            Self::Zrle => "zrle",
            Self::Tight => "tight",
            Self::TightJpeg => "tight-jpeg-q8",
        }
    }

    fn encoding(self) -> Encoding {
        match self {
            Self::Raw => Encoding::Raw,
            Self::Hextile => Encoding::Hextile,
            Self::Zrle => Encoding::Zrle,
            Self::Tight | Self::TightJpeg => Encoding::Tight,
        }
    }
}

fn encode_frame(codec: Codec, fb: &Framebuffer, conv: &PixelConverter) -> Vec<(Rect, Vec<u8>)> {
    let mut zrle = ZrleEncoder::new(2);
    let mut tight = TightEncoder::new(2);
    pieces()
        .into_iter()
        .map(|rect| {
            let payload = match codec {
                Codec::Raw => encode::raw(fb, rect, conv).unwrap(),
                Codec::Hextile => encode::hextile(fb, rect, conv).unwrap(),
                Codec::Zrle => zrle.encode(fb, rect, conv).unwrap(),
                Codec::Tight => tight.encode(fb, rect, conv).unwrap(),
                Codec::TightJpeg => tight.encode_jpeg(fb, rect, conv, 8).unwrap().unwrap(),
            };
            (rect, payload)
        })
        .collect()
}

fn bench(c: &mut Criterion) {
    let conv = PixelConverter::new(PixelFormat::RGB888);
    for (content, fb) in [("desktop", desktop()), ("photo", photo())] {
        let mut group = c.benchmark_group(format!("1080p-{content}"));
        group.throughput(Throughput::Elements(1));
        group.sample_size(10);
        for codec in Codec::ALL {
            group.bench_function(format!("encode/{}", codec.name()), |b| {
                b.iter(|| black_box(encode_frame(codec, &fb, &conv)));
            });
            let encoded = encode_frame(codec, &fb, &conv);
            let bytes: usize = encoded.iter().map(|(_, p)| p.len()).sum();
            eprintln!("{content} {}: {} KiB per frame", codec.name(), bytes / 1024);
            group.bench_function(format!("decode/{}", codec.name()), |b| {
                b.iter(|| {
                    let mut decoder = Decoder::new(W, H, PixelFormat::RGB888, Limits::DEFAULT);
                    for (rect, payload) in &encoded {
                        let header = RectHeader {
                            rect: *rect,
                            encoding: codec.encoding(),
                        };
                        decoder.decode_rect(&header, payload).unwrap();
                    }
                    black_box(decoder);
                });
            });
        }
        group.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
