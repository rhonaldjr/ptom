#![no_main]

use libfuzzer_sys::fuzz_target;
use farsight_rfb::PixelFormat;

fuzz_target!(|data: &[u8]| {
    if let Ok(format) = PixelFormat::decode(data) {
        assert_eq!(PixelFormat::decode(&format.encode()), Ok(format));
    }
});
