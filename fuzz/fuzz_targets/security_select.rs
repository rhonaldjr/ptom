#![no_main]

use libfuzzer_sys::fuzz_target;
use farsight_rfb::SecurityType;
use farsight_security::SecurityPolicy;

fuzz_target!(|data: &[u8]| {
    let offered: Vec<_> = data.iter().copied().map(SecurityType::from_code).collect();
    if let Some(chosen) = SecurityPolicy::NETWORK.select(&offered) {
        assert!(!chosen.is_legacy_insecure());
        assert!(offered.contains(&chosen));
    }
});
