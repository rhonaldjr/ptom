#![no_main]

use farsight_rfb::{Limits, ServerMessage};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((message, used)) = ServerMessage::decode(data, &Limits::DEFAULT) {
        assert!(used <= data.len());
        let mut out = Vec::new();
        if message.encode(&mut out).is_ok() {
            assert_eq!(ServerMessage::decode(&out, &Limits::DEFAULT), Ok((message, out.len())));
        }
    }
});
