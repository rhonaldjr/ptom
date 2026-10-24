#![no_main]
//! A malicious viewer: arbitrary bytes into the agent's `ServerSession`
//! (None and VncAuth offered, prompts auto-approved), with framebuffer
//! updates, cursor and clipboard flowing back after every chunk.
//!
//! Byte 0 picks options: bit 0 prepends a valid None handshake so the
//! fuzzer starts in the normal phase; bit 1 makes the session view-only;
//! bits 2–3 pick the agent's encodings. Byte 1 is the chunk size.

use std::sync::Arc;

use farsight_codec::Cursor;
use farsight_rfb::{Encoding, Rect, Screen, SecurityType};
use farsight_server::{
    ClipboardSink, Desktop, DesktopResizer, Entropy, InputSink, ResizeOutcome, ServerSession,
    ServerSessionConfig, SessionEvent, SessionPorts,
};
use libfuzzer_sys::fuzz_target;
use secrecy::SecretString;

struct Fixed;
impl Entropy for Fixed {
    fn fill(&self, buf: &mut [u8]) {
        buf.fill(7);
    }
}

struct Sink;
impl InputSink for Sink {
    fn key(&mut self, _: bool, _: u32) {}
    fn pointer(&mut self, _: u8, _: u16, _: u16) {}
}
impl ClipboardSink for Sink {
    fn set_text(&mut self, _: &str) {}
}
impl DesktopResizer for Sink {
    fn resize(&mut self, width: u16, height: u16, _: &[Screen]) -> ResizeOutcome {
        if width > 0 && height > 0 && width <= 256 && height <= 256 {
            ResizeOutcome::Accepted
        } else {
            ResizeOutcome::InvalidLayout
        }
    }
}

const HANDSHAKE: &[u8] = b"RFB 003.008\n\x01\x01";

fuzz_target!(|data: &[u8]| {
    let [options, chunk, rest @ ..] = data else {
        return;
    };
    let mut config = ServerSessionConfig::new("fuzz", Arc::new(Fixed));
    config.loopback = true;
    config.security_types = vec![SecurityType::None, SecurityType::VncAuth];
    config.vnc_password = Some(SecretString::from("pw".to_owned()));
    config.view_only = options & 2 != 0;
    config.encodings = match (options >> 2) & 3 {
        0 => vec![
            Encoding::Tight,
            Encoding::Zrle,
            Encoding::Hextile,
            Encoding::Rre,
            Encoding::Raw,
        ],
        1 => vec![Encoding::Zrle],
        2 => vec![Encoding::Raw],
        _ => vec![Encoding::Tight],
    };
    let ports = SessionPorts {
        input: Box::new(Sink),
        clipboard: Box::new(Sink),
        resizer: Some(Box::new(Sink)),
    };
    let mut session = ServerSession::new(config, ports);
    let mut desktop = Desktop::new(80, 60, "fuzz");
    desktop.set_cursor(Cursor::from_rgba(2, 2, (1, 1), vec![255; 16]));

    let mut input = Vec::new();
    if options & 1 != 0 {
        input.extend_from_slice(HANDSHAKE);
    }
    input.extend_from_slice(rest);
    for (n, piece) in input.chunks(usize::from(*chunk).max(1)).enumerate() {
        session.receive(piece);
        let Ok(events) = session.poll() else {
            return;
        };
        for event in events {
            let result = match event {
                SessionEvent::ClientInit { .. } => session.admit(&desktop),
                SessionEvent::AccessRequest { .. } => session.allow(&desktop),
                _ => Ok(()),
            };
            if result.is_err() {
                return;
            }
        }
        // Keep the desktop changing: damage, resize, clipboard.
        let n = u16::try_from(n % 50).unwrap_or(0);
        let _ = desktop.update_region(Rect::new(n, n, 3, 3), &[u32::from(n) * 999; 9], None);
        if n == 7 {
            desktop.resize(64 + n, 48);
        }
        if n == 9 {
            desktop.set_clipboard("clip");
        }
        // Encoding the agent's own desktop must never fail.
        session.frame_ready(&desktop).expect("frame_ready");
        let _ = session.take_output();
    }
});
