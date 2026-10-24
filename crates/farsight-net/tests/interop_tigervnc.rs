//! Interop (Q4) against a real TigerVNC server. Opt-in: set
//! `FARSIGHT_INTEROP_ADDR` (e.g. `127.0.0.1::5901`) and optionally
//! `FARSIGHT_INTEROP_PASSWORD`. CI starts `tests/interop/tigervnc`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use farsight_client::{ServerAddress, SessionConfig, SessionEvent};
use farsight_net::{Command, Update, connect};
use secrecy::SecretString;

#[tokio::test]
async fn connects_authenticates_and_decodes_a_red_desktop() {
    let Ok(address) = std::env::var("FARSIGHT_INTEROP_ADDR") else {
        eprintln!("skipped: FARSIGHT_INTEROP_ADDR not set");
        return;
    };
    let password = std::env::var("FARSIGHT_INTEROP_PASSWORD").unwrap_or_else(|_| "farsight".into());
    let address = ServerAddress::parse(&address).unwrap();
    let (handle, mut updates) = connect(
        &address,
        SessionConfig {
            shared: true,
            ..SessionConfig::default()
        },
    )
    .await
    .unwrap();

    let deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(deadline);
    let mut connected = None;
    loop {
        let update = tokio::select! {
            () = &mut deadline => panic!("timed out; connected = {connected:?}"),
            update = updates.recv() => update.expect("connection task ended"),
        };
        match update {
            Update::Event(SessionEvent::InsecureConsentRequired { .. }) => {
                assert!(handle.send(Command::ConsentInsecure));
            }
            Update::Event(SessionEvent::PasswordRequired) => {
                assert!(handle.send(Command::Password(SecretString::from(password.clone()))));
            }
            Update::Event(SessionEvent::Connected { width, height, .. }) => {
                connected = Some((width, height));
            }
            Update::Frame(frame) => {
                assert_eq!(connected, Some((640, 480)));
                assert_eq!((frame.width, frame.height), (640, 480));
                let centre = (240 * 640 + 320) * 4;
                if frame.rgba.get(centre..centre + 3) == Some(&[0xff, 0, 0]) {
                    assert!(handle.send(Command::Disconnect));
                    return;
                }
            }
            Update::Closed(reason) => panic!("closed: {reason:?}"),
            Update::Event(_) | Update::Cursor(_) => {}
        }
    }
}
