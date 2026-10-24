//! Q1 property tests: every server-side encoder's output decodes, through the
//! viewer's public `Decoder`, to exactly the pixels a Raw rectangle would
//! give, in RGB888 and RGB565 (both byte orders), across sequences of
//! rectangles that share one zlib stream.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use farsight_codec::encode::{self, TightEncoder, ZrleEncoder};
use farsight_codec::{Cursor, Decoder, Framebuffer, PixelConverter};
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Encoding, Limits, PixelFormat, Rect, RectHeader, Screen};
use proptest::prelude::*;

const RGB565_BE: PixelFormat = PixelFormat {
    bits_per_pixel: 16,
    depth: 16,
    big_endian: true,
    true_colour: true,
    red_max: 31,
    green_max: 63,
    blue_max: 31,
    red_shift: 11,
    green_shift: 5,
    blue_shift: 0,
};

const RGB565_LE: PixelFormat = PixelFormat {
    big_endian: false,
    ..RGB565_BE
};

/// BGR, big-endian, shifted into the high bytes: exercises the other CPIXEL
/// layout.
const BGR888_HIGH_BE: PixelFormat = PixelFormat {
    big_endian: true,
    red_shift: 8,
    green_shift: 16,
    blue_shift: 24,
    ..PixelFormat::RGB888
};

fn formats() -> impl Strategy<Value = PixelFormat> {
    prop_oneof![
        Just(PixelFormat::RGB888),
        Just(RGB565_BE),
        Just(RGB565_LE),
        Just(BGR888_HIGH_BE),
    ]
}

/// A framebuffer drawn from a palette of 1 to 300 colours, laid out in
/// runs, so solid, palette, RLE and raw paths all get exercised.
fn framebuffers() -> impl Strategy<Value = Framebuffer> {
    (1u16..150, 1u16..100, 1usize..300, any::<u64>(), 1usize..40).prop_map(
        |(w, h, colours, seed, run)| {
            let mut fb = Framebuffer::new(w, h);
            let mut state = seed | 1;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let palette: Vec<u32> = (0..colours)
                .map(|_| u32::try_from(next() & 0xff_ffff).unwrap())
                .collect();
            let mut current = palette[0];
            fb.write(Rect::new(0, 0, w, h), |i| {
                if i % run == 0 {
                    current = palette[usize::try_from(next()).unwrap() % palette.len()];
                }
                current
            })
            .unwrap();
            fb
        },
    )
}

fn rects_in(w: u16, h: u16) -> impl Strategy<Value = Vec<Rect>> {
    proptest::collection::vec(
        (0..w, 0..h, 1u16..=w, 1u16..=h)
            .prop_map(move |(x, y, rw, rh)| Rect::new(x, y, rw.min(w - x), rh.min(h - y))),
        1..5,
    )
}

/// What the client sees for `rect` when it is sent as Raw.
fn expected(fb: &Framebuffer, rect: Rect, format: PixelFormat) -> Vec<u32> {
    let payload = encode::raw(fb, rect, &PixelConverter::new(format)).unwrap();
    let mut d = Decoder::new(fb.width(), fb.height(), format, Limits::DEFAULT);
    let header = RectHeader {
        rect,
        encoding: Encoding::Raw,
    };
    d.decode_rect(&header, &payload).unwrap().unwrap();
    d.framebuffer().read(rect).unwrap()
}

#[derive(Debug, Clone, Copy)]
enum Codec {
    Raw,
    Zrle,
    Tight,
}

fn check_round_trip(
    codec: Codec,
    fb: &Framebuffer,
    rects: &[Rect],
    format: PixelFormat,
    level: u8,
) -> Result<(), TestCaseError> {
    let conv = PixelConverter::new(format);
    let mut zrle = ZrleEncoder::new(level);
    let mut tight = TightEncoder::new(level);
    let mut d = Decoder::new(fb.width(), fb.height(), format, Limits::DEFAULT);
    for (n, &rect) in rects.iter().enumerate() {
        if n == 2 {
            tight.set_compress_level(9 - level);
        }
        let (encoding, payload) = match codec {
            Codec::Raw => (Encoding::Raw, encode::raw(fb, rect, &conv).unwrap()),
            Codec::Zrle => (Encoding::Zrle, zrle.encode(fb, rect, &conv).unwrap()),
            Codec::Tight => (Encoding::Tight, tight.encode(fb, rect, &conv).unwrap()),
        };
        let header = RectHeader { rect, encoding };
        // Never applied early: every proper prefix waits for more input.
        for cut in [0, payload.len() / 2, payload.len().saturating_sub(1)] {
            if cut < payload.len() {
                prop_assert_eq!(d.decode_rect(&header, &payload[..cut]), Ok(None));
            }
        }
        prop_assert_eq!(d.decode_rect(&header, &payload), Ok(Some(payload.len())));
        prop_assert_eq!(
            d.framebuffer().read(rect).unwrap(),
            expected(fb, rect, format),
            "{:?} rect {} {:?}",
            codec,
            n,
            rect
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn zrle_round_trips(
        (fb, rects) in framebuffers().prop_flat_map(|fb| { let r = rects_in(fb.width(), fb.height()); (Just(fb), r) }),
        format in formats(),
        level in 0u8..=9,
    ) {
        check_round_trip(Codec::Zrle, &fb, &rects, format, level)?;
    }

    #[test]
    fn tight_round_trips(
        (fb, rects) in framebuffers().prop_flat_map(|fb| { let r = rects_in(fb.width(), fb.height()); (Just(fb), r) }),
        format in formats(),
        level in 0u8..=9,
    ) {
        check_round_trip(Codec::Tight, &fb, &rects, format, level)?;
    }

    #[test]
    fn raw_round_trips(
        (fb, rects) in framebuffers().prop_flat_map(|fb| { let r = rects_in(fb.width(), fb.height()); (Just(fb), r) }),
        format in formats(),
    ) {
        check_round_trip(Codec::Raw, &fb, &rects, format, 0)?;
    }

    #[test]
    fn cursors_round_trip(
        (w, h, hx, hy) in (0u16..40, 0u16..40, 0u16..50, 0u16..50),
        seed in any::<u32>(),
        format in formats(),
    ) {
        // Fully opaque or fully transparent pixels survive both pseudo-encodings.
        let rgba: Vec<u8> = (0..usize::from(w) * usize::from(h))
            .flat_map(|i| {
                let v = u32::try_from(i).unwrap().wrapping_mul(seed | 1).wrapping_add(seed);
                let [r, g, b, a] = v.to_le_bytes();
                if a & 1 == 0 { [r, g, b, 255] } else { [0, 0, 0, 0] }
            })
            .collect();
        let cursor = Cursor::from_rgba(w, h, (hx, hy), rgba).unwrap();
        let header = |encoding| RectHeader {
            rect: Rect::new(cursor.hotspot().0, cursor.hotspot().1, w, h),
            encoding,
        };
        let mut d = Decoder::new(8, 8, format, Limits::DEFAULT);
        let payload = encode::cursor_with_alpha(&cursor);
        prop_assert_eq!(d.decode_rect(&header(Encoding::CursorWithAlpha), &payload), Ok(Some(payload.len())));
        prop_assert_eq!(d.cursor(), Some(&cursor));

        // Cursor quantises colours to the client format, like Raw.
        let conv = PixelConverter::new(format);
        let payload = encode::cursor(&cursor, &conv);
        prop_assert_eq!(d.decode_rect(&header(Encoding::Cursor), &payload), Ok(Some(payload.len())));
        let decoded = d.cursor().unwrap();
        prop_assert_eq!(decoded.hotspot(), cursor.hotspot());
        for (got, want) in decoded.rgba().chunks(4).zip(cursor.rgba().chunks(4)) {
            prop_assert_eq!(got[3], want[3]);
            if want[3] == 255 {
                let rgb = (u32::from(want[0]) << 16) | (u32::from(want[1]) << 8) | u32::from(want[2]);
                let mut wire = Vec::new();
                conv.encode(rgb, &mut wire);
                let [_, r, g, b] = conv.decode(&wire).to_be_bytes();
                prop_assert_eq!(&got[..3], &[r, g, b][..]);
            }
        }
    }
}

#[test]
fn extended_desktop_size_round_trips() {
    let screens = [Screen {
        id: 7,
        x: 0,
        y: 0,
        width: 640,
        height: 480,
        flags: 0,
    }];
    let payload = encode::extended_desktop_size(&screens);
    let mut d = Decoder::new(8, 8, PixelFormat::RGB888, Limits::DEFAULT);
    let header = RectHeader {
        rect: Rect::new(0, 0, 640, 480),
        encoding: Encoding::ExtendedDesktopSize,
    };
    assert_eq!(d.decode_rect(&header, &payload), Ok(Some(payload.len())));
    assert_eq!(d.screens(), screens);
    assert_eq!(d.framebuffer().width(), 640);
}

#[test]
fn cursor_rejects_bad_shapes() {
    assert!(Cursor::from_rgba(2, 2, (0, 0), vec![0; 15]).is_none());
    assert!(Cursor::from_rgba(257, 1, (0, 0), vec![0; 257 * 4]).is_none());
    let c = Cursor::from_rgba(2, 2, (9, 9), vec![0; 16]).unwrap();
    assert_eq!(c.hotspot(), (1, 1));
}
