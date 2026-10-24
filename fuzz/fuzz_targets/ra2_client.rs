#![no_main]
//! A malicious RSA-AES server: arbitrary bytes into the viewer's authenticator.

use farsight_rfb::auth::{AuthProgress, AuthRequest, AuthResponse, Authenticator};
use farsight_security::ra2::{Ra2Client, Ra2Variant};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&flag, data)) = data.split_first() else { return };
    let variant = if flag & 1 == 0 { Ra2Variant::Sha1 } else { Ra2Variant::Sha256 };
    let mut client = Ra2Client::new(variant, None, None);
    let mut out = Vec::new();
    let mut rest = data;
    for _ in 0..8 {
        match client.advance(rest, &mut out) {
            Ok((used, progress)) => {
                assert!(used <= rest.len());
                rest = &rest[used..];
                match progress {
                    AuthProgress::Paused(AuthRequest::ConfirmServerKey { .. }) => {
                        if client.respond(AuthResponse::ServerKeyAccepted).is_err() {
                            return;
                        }
                    }
                    AuthProgress::NeedMore if used == 0 => return,
                    AuthProgress::Complete | AuthProgress::Paused(_) => return,
                    _ => {}
                }
            }
            Err(_) => return,
        }
    }
});
