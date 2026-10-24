//! mDNS advertise + browse round trip (ADR-0010).
//!
//! Ignored by default: it needs multicast on a real interface (container and
//! CI sandboxes often have none), binds UDP 5353, and can see other machines'
//! records. Run with `cargo test -p farsight-discovery --test mdns -- --ignored`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::{Duration, Instant};

use farsight_discovery::{
    AdvertiseConfig, BrowseEvent, Browser, Provenance, Verification, advertise,
};
use tokio::time::timeout;

const FP: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
/// Council condition: advertised servers appear within 5 s.
const APPEAR_WITHIN: Duration = Duration::from_secs(5);

#[tokio::test]
#[ignore = "needs multicast networking"]
async fn advertised_instance_is_browsed_within_five_seconds() {
    let unique = format!("farsight-test-{}", std::process::id());
    let advertisement = advertise(&AdvertiseConfig {
        instance_name: unique.clone(),
        host_name: unique.clone(),
        port: 5999,
        fingerprint: FP.to_owned(),
        addresses: Vec::new(),
    })
    .unwrap();
    let mut browser = Browser::start().unwrap();

    let started = Instant::now();
    let found = loop {
        let remaining = APPEAR_WITHIN.saturating_sub(started.elapsed());
        let Ok(event) = timeout(remaining, browser.next_event()).await else {
            panic!("instance did not appear within {APPEAR_WITHIN:?}");
        };
        match event {
            Some(BrowseEvent::Resolved(server)) if server.name == unique => break server,
            Some(_) => {}
            None => panic!("browse ended"),
        }
    };

    assert_eq!(found.provenance, Provenance::Mdns);
    assert_eq!(found.verification, Verification::Unverified);
    assert_eq!(found.port, 5999);
    assert_ne!(found.addresses, Vec::<std::net::IpAddr>::new());
    assert!(found.txt.farsight);
    assert_eq!(found.txt.version.as_deref(), Some("3.8"));
    assert_eq!(found.txt.fingerprint.as_deref(), Some(FP));

    let key = found.key.clone();
    drop(advertisement);
    let removed = timeout(APPEAR_WITHIN, async {
        loop {
            match browser.next_event().await {
                Some(BrowseEvent::Removed(gone)) if gone == key => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await;
    assert_eq!(removed, Ok(true), "the goodbye should remove the instance");
}
