#![no_main]
//! A malicious VeNCrypt server: arbitrary bytes into the viewer's authenticator.

use farsight_rfb::auth::{AuthProgress, AuthRequest, AuthResponse, Authenticator};
use farsight_security::vencrypt::VeNCryptClient;
use libfuzzer_sys::fuzz_target;
use secrecy::SecretString;

fuzz_target!(|data: &[u8]| {
    let mut client = VeNCryptClient::new(None, None);
    let mut pending = Vec::new();
    let mut out = Vec::new();
    for chunk in data.chunks(3) {
        pending.extend_from_slice(chunk);
        loop {
            match client.advance(&pending, &mut out) {
                Ok((used, progress)) => {
                    assert!(used <= pending.len());
                    pending.drain(..used);
                    match progress {
                        AuthProgress::Paused(AuthRequest::Credentials { .. }) => {
                            let response = AuthResponse::Credentials {
                                username: Some(SecretString::from("u".to_owned())),
                                password: SecretString::from("p".to_owned()),
                            };
                            if client.respond(response).is_err() {
                                return;
                            }
                        }
                        AuthProgress::Complete => return,
                        AuthProgress::UpgradeTransport(_) => {}
                        _ => break,
                    }
                    if used == 0 {
                        break;
                    }
                }
                Err(_) => return,
            }
        }
    }
});
