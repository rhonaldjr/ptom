//! Q4 benchmarks (M8): the whole agent → viewer path in memory (no network,
//! no capture): a change on the desktop until the viewer has the pixels.
//! This is the latency Farsight itself adds on top of the network.
//!
//! `cargo bench -p farsight-server`; results are summarised in
//! docs/performance/baselines.md.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names
)]

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use farsight_client::{Session, SessionConfig, SessionEvent as ViewerEvent};
use farsight_codec::Framebuffer;
use farsight_rfb::{Rect, SecurityType};
use farsight_server::{
    ClipboardSink, Desktop, Entropy, InputSink, ServerSession, ServerSessionConfig, SessionEvent,
    SessionPorts,
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

struct Pair {
    server: ServerSession,
    viewer: Session,
    desktop: Desktop,
}

impl Pair {
    fn connect(width: u16, height: u16, jpeg: bool) -> Self {
        let mut config = ServerSessionConfig::new("127.0.0.1:1", Arc::new(Zero));
        config.loopback = true;
        config.security_types = vec![SecurityType::None];
        // Nobody to ask in a benchmark.
        config.prompt_non_owner = false;
        let ports = SessionPorts {
            input: Box::new(Ignore),
            clipboard: Box::new(Ignore),
            resizer: None,
        };
        let mut pair = Self {
            server: ServerSession::new(config, ports),
            viewer: Session::new(SessionConfig {
                loopback: true,
                shared: true,
                no_jpeg: !jpeg,
                ..SessionConfig::default()
            }),
            desktop: Desktop::new(width, height, "bench"),
        };
        pair.desktop.update(desktop_frame(width, height, 0), None);
        pair.exchange();
        pair
    }

    /// Moves bytes both ways until the viewer has finished an update.
    fn exchange(&mut self) -> bool {
        let mut updated = false;
        for _ in 0..20 {
            self.server.frame_ready(&self.desktop).unwrap();
            self.viewer.receive(&self.server.take_output());
            for event in self.viewer.poll().unwrap() {
                updated |= matches!(event, ViewerEvent::UpdateComplete);
            }
            self.server.receive(&self.viewer.take_output());
            for event in self.server.poll().unwrap() {
                if let SessionEvent::ClientInit { .. } = event {
                    self.server.admit(&self.desktop).unwrap();
                }
            }
            if updated {
                return true;
            }
        }
        updated
    }
}

/// A desktop-like frame; `n` moves a "window" so frames differ.
fn desktop_frame(width: u16, height: u16, n: usize) -> Framebuffer {
    let mut fb = Framebuffer::new(width, height);
    fb.write(Rect::new(0, 0, width, height), |i| {
        let (x, y) = (i % usize::from(width), i / usize::from(width));
        if (x + n * 8) % 600 < 400 && (y / 18) % 2 == 0 && (x * 7 + y * 3) % 11 < 4 {
            0x0010_1010
        } else {
            0x00f0_f0f0
        }
    })
    .unwrap();
    fb
}

fn bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("agent-to-viewer");
    group.sample_size(20);

    // Typing: a 16×16 change (one character) reaches the viewer.
    let mut pair = Pair::connect(1920, 1080, true);
    let mut n = 0u32;
    group.bench_function("small-change-1080p", |b| {
        b.iter(|| {
            n = n.wrapping_add(1);
            let mut frame = pair.desktop.framebuffer().clone();
            frame
                .fill(Rect::new(100, 100, 16, 16), n & 0x00ff_ffff)
                .unwrap();
            pair.desktop
                .update(frame, Some(&[Rect::new(100, 100, 16, 16)]));
            assert!(pair.exchange());
        });
    });

    // Scrolling or switching windows: the whole 1080p desktop changes.
    let mut pair = Pair::connect(1920, 1080, true);
    let mut n = 0usize;
    group.bench_function("full-frame-1080p", |b| {
        b.iter(|| {
            n += 1;
            pair.desktop.update(desktop_frame(1920, 1080, n), None);
            assert!(pair.exchange());
        });
    });
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
