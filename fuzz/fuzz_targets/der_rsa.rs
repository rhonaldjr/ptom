#![no_main]

use farsight_security::der;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some((modulus, exponent)) = der::rsa_components(data) {
        let again = der::rsa_spki(&modulus, &exponent);
        assert_eq!(der::rsa_components(&again), Some((modulus, exponent)));
    }
});
