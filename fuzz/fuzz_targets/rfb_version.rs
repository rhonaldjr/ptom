#![no_main]

use libfuzzer_sys::fuzz_target;
use ptom_rfb::ProtocolVersion;

fuzz_target!(|data: &[u8]| {
    if let Ok(version) = ProtocolVersion::decode(data) {
        assert_eq!(ProtocolVersion::decode(&version.to_bytes()), Ok(version));
    }
});
