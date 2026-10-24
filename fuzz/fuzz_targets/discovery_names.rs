#![no_main]

//! Peer-supplied discovery text: instance names, TXT records and the /proc
//! parsers. Nothing may panic, and no hidden character may survive sanitising.

use farsight_discovery::mdns::instance_name;
use farsight_discovery::procfs::{parse_arp, parse_routes};
use farsight_discovery::sanitize::{MAX_NAME_CHARS, is_hidden};
use farsight_discovery::{SERVICE_TYPE, parse_txt, sanitize_name};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    let clean = sanitize_name(&text);
    assert!(clean.chars().count() <= MAX_NAME_CHARS);
    assert!(!clean.chars().any(is_hidden));
    assert_eq!(sanitize_name(&clean), clean);

    let instance = instance_name(&text, SERVICE_TYPE);
    assert!(!instance.chars().any(is_hidden));

    // TXT entries as `key=value` strings split on 0xff, like a decoded record.
    let entries: Vec<(&[u8], Option<&[u8]>)> = data
        .split(|b| *b == 0xff)
        .map(|entry| match entry.iter().position(|b| *b == b'=') {
            Some(eq) => (&entry[..eq], Some(&entry[eq + 1..])),
            None => (entry, None),
        })
        .collect();
    let info = parse_txt(
        entries
            .iter()
            .filter_map(|(k, v)| Some((std::str::from_utf8(k).ok()?, *v))),
    );
    if let Some(fp) = info.fingerprint {
        assert!(fp.len() == 64 && fp.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    let _ = parse_routes(&text);
    let _ = parse_arp(&text);
});
