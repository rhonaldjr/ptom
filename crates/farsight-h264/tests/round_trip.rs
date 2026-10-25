//! Q2/Q4: Cisco's library is downloaded and verified, then frames encoded by
//! the agent's side decode, through the viewer's `Decoder` and the Open
//! H.264 framing, to nearly the same pixels. Needs network access to
//! ciscobinary.openh264.org; skipped (with a message) without it.

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use farsight_codec::video::{RESET_CONTEXT, VideoCodecs, open_h264_payload};
use farsight_codec::{Decoder, Framebuffer};
use farsight_h264::{H264Error, OpenH264, install};
use farsight_rfb::client::RectDecoder;
use farsight_rfb::{Encoding, Limits, PixelFormat, Rect, RectHeader};

fn codecs() -> Option<OpenH264> {
    let dir = tempfile::tempdir().unwrap().keep();
    match install(&dir) {
        Ok(_) => Some(OpenH264::load(&dir).unwrap()),
        Err(H264Error::Download(reason)) => {
            eprintln!("skipping: can't download OpenH264 ({reason})");
            None
        }
        Err(H264Error::UnsupportedPlatform) => {
            eprintln!("skipping: Cisco has no OpenH264 build for this computer");
            None
        }
        Err(other) => panic!("{other}"),
    }
}

fn frame(w: u16, h: u16, shift: usize) -> Vec<u32> {
    (0..usize::from(w) * usize::from(h))
        .map(|i| {
            let (x, y) = (i % usize::from(w) + shift, i / usize::from(w));
            if (x / 20 + y / 20) % 2 == 0 {
                0x00d0_3030
            } else {
                0x0030_60d0
            }
        })
        .collect()
}

#[test]
fn encoded_frames_decode_close_to_the_original() {
    let Some(codecs) = codecs() else { return };
    let (w, h) = (160, 96);
    let mut encoder = codecs.encoder(w, h).unwrap();
    let mut viewer = Decoder::new(w, h, PixelFormat::RGB888, Limits::DEFAULT);
    viewer.set_video(Arc::new(codecs.clone()));
    let header = RectHeader {
        rect: Rect::new(0, 0, w, h),
        encoding: Encoding::OpenH264,
    };
    for (n, shift) in [0, 4, 8].into_iter().enumerate() {
        let pixels = frame(w, h, shift);
        let data = encoder.encode(&pixels).unwrap();
        let flags = if n == 0 { RESET_CONTEXT } else { 0 };
        let payload = open_h264_payload(flags, &data);
        assert_eq!(
            viewer.decode_rect(&header, &payload[..7]),
            Ok(None),
            "waits for the header"
        );
        assert_eq!(
            viewer.decode_rect(&header, &payload),
            Ok(Some(payload.len()))
        );
        let mut original = Framebuffer::new(w, h);
        original
            .write(Rect::new(0, 0, w, h), |i| pixels[i])
            .unwrap();
        let worst = original
            .pixels()
            .iter()
            .zip(viewer.framebuffer().pixels())
            .flat_map(|(a, b)| {
                (0..3).map(move |c| ((a >> (c * 8)) & 0xff).abs_diff((b >> (c * 8)) & 0xff))
            })
            .max()
            .unwrap();
        assert!(worst < 96, "frame {n}: worst channel error {worst}");
    }
}

#[test]
fn odd_sizes_and_garbage_are_refused_safely() {
    let Some(codecs) = codecs() else { return };
    assert!(codecs.encoder(161, 96).is_err());
    let mut viewer = Decoder::new(16, 16, PixelFormat::RGB888, Limits::DEFAULT);
    viewer.set_video(Arc::new(codecs));
    let header = RectHeader {
        rect: Rect::new(0, 0, 16, 16),
        encoding: Encoding::OpenH264,
    };
    // Garbage frames never panic; at worst they are an error.
    let _ = viewer.decode_rect(
        &header,
        &open_h264_payload(0, &[0, 0, 0, 1, 0x65, 0xff, 0x13]),
    );
    let huge = (17u32 << 20).to_be_bytes();
    assert!(
        viewer
            .decode_rect(&header, &[huge.as_slice(), &[0; 4]].concat())
            .is_err()
    );
}

mod session {
    use std::sync::Arc;

    use farsight_client::{Session, SessionConfig, SessionEvent as ViewerEvent};
    use farsight_codec::Framebuffer;
    use farsight_codec::video::SharedCodecs;
    use farsight_rfb::{Rect, SecurityType};
    use farsight_server::{
        ClipboardSink, Desktop, Entropy, InputSink, ServerSession, ServerSessionConfig,
        SessionEvent, SessionPorts,
    };

    struct Zero;
    impl Entropy for Zero {
        fn fill(&self, buf: &mut [u8]) {
            buf.fill(0);
        }
    }
    struct Ignore;
    impl InputSink for Ignore {
        fn key(&mut self, _: bool, _: u32) {}
        fn pointer(&mut self, _: u8, _: u16, _: u16) {}
    }
    impl ClipboardSink for Ignore {
        fn set_text(&mut self, _: &str) {}
    }

    /// The agent sends the desktop as H.264 to a viewer that can decode it.
    #[test]
    fn agent_and_viewer_stream_the_desktop_as_h264() {
        let Some(codecs) = super::codecs() else {
            return;
        };
        let codecs = Arc::new(codecs);
        let mut config = ServerSessionConfig::new("127.0.0.1:1", Arc::new(Zero));
        config.loopback = true;
        config.security_types = vec![SecurityType::None];
        config.prompt_non_owner = false;
        config.video = Some(codecs.clone());
        let ports = SessionPorts {
            input: Box::new(Ignore),
            clipboard: Box::new(Ignore),
            resizer: None,
        };
        let mut server = ServerSession::new(config, ports);
        let mut viewer = Session::new(SessionConfig {
            loopback: true,
            shared: true,
            video: Some(SharedCodecs(codecs)),
            ..SessionConfig::default()
        });
        let (w, h) = (320, 200);
        let mut desktop = Desktop::new(w, h, "h264");
        let mut frame = Framebuffer::new(w, h);
        frame
            .write(Rect::new(0, 0, w, h), |i| {
                if (i % 320) < 160 {
                    0x0020_4080
                } else {
                    0x00e0_e0e0
                }
            })
            .unwrap();
        desktop.update(frame.clone(), None);
        let mut updates = 0;
        for _ in 0..30 {
            server.frame_ready(&desktop).unwrap();
            viewer.receive(&server.take_output());
            for event in viewer.poll().unwrap() {
                updates += usize::from(matches!(event, ViewerEvent::UpdateComplete));
            }
            server.receive(&viewer.take_output());
            for event in server.poll().unwrap() {
                if let SessionEvent::ClientInit { .. } = event {
                    server.admit(&desktop).unwrap();
                }
            }
        }
        assert!(updates > 0, "the viewer got the desktop");
        let worst = frame
            .pixels()
            .iter()
            .zip(viewer.framebuffer().pixels())
            .flat_map(|(a, b)| {
                (0..3).map(move |c| ((a >> (c * 8)) & 0xff).abs_diff((b >> (c * 8)) & 0xff))
            })
            .max()
            .unwrap();
        assert!(worst < 64, "close to the desktop (worst {worst})");
    }
}

/// Regression (M10 review): a picture larger than its rectangle is refused
/// before the viewer converts (allocates) it.
#[test]
fn a_picture_of_the_wrong_size_is_refused() {
    let Some(codecs) = codecs() else { return };
    let mut encoder = codecs.encoder(160, 96).unwrap();
    let data = encoder.encode(&frame(160, 96, 0)).unwrap();
    let mut decoder = codecs.decoder().unwrap();
    assert!(decoder.decode(&data, (80, 48)).is_err());
    let mut decoder = codecs.decoder().unwrap();
    assert!(decoder.decode(&data, (160, 96)).unwrap().is_some());
}
