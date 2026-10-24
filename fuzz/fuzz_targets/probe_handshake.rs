#![no_main]

//! The active prober's handshake reader (ADR-0012) against hostile servers.
//! Besides never panicking, it must never ask to send anything but a single
//! 12-byte version message, and nothing at all once it has finished.

use farsight_discovery::{HandshakeStep, ProbeHandshake};
use libfuzzer_sys::fuzz_target;

const VERSIONS: [&[u8]; 3] = [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"];

fuzz_target!(|data: &[u8]| {
    // The first byte picks chunk sizes so split reads are covered.
    let Some((&split, rest)) = data.split_first() else {
        return;
    };
    let chunk = usize::from(split % 32) + 1;
    let mut handshake = ProbeHandshake::new();
    let mut sent = Vec::new();
    let mut finished = false;
    for piece in rest.chunks(chunk) {
        match handshake.feed(piece) {
            HandshakeStep::Continue { reply } => {
                assert!(!finished, "continued after finishing");
                sent.extend(reply);
            }
            HandshakeStep::Finished(_) => finished = true,
        }
    }
    assert!(
        sent.is_empty() || VERSIONS.contains(&sent.as_slice()),
        "sent {sent:?}"
    );
});
