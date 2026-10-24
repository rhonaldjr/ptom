#![no_main]
//! A malicious client: arbitrary bytes into the agent's state machine.

use farsight_rfb::auth::{Authenticator, NoAuth};
use farsight_rfb::handshake::ServerInit;
use farsight_rfb::server::{ServerConfig, ServerConnection, ServerEvent};
use farsight_rfb::{Limits, PixelFormat, ProtocolVersion, SecurityType};
use farsight_security::vnc_auth::VncAuthServer;
use libfuzzer_sys::fuzz_target;
use secrecy::SecretString;

fuzz_target!(|data: &[u8]| {
    let mut server = ServerConnection::new(ServerConfig {
        max_version: ProtocolVersion::V3_8,
        security_types: vec![SecurityType::None, SecurityType::VncAuth],
        limits: Limits::DEFAULT,
    });
    for chunk in data.chunks(5) {
        server.receive(chunk);
        loop {
            match server.poll() {
                Ok(Some(ServerEvent::SecuritySelected(security))) => {
                    let auth: Box<dyn Authenticator> = if security == SecurityType::VncAuth {
                        Box::new(VncAuthServer::new(SecretString::from("pw".to_owned()), [3; 16]))
                    } else {
                        Box::new(NoAuth)
                    };
                    if server.start_authentication(security, auth).is_err() {
                        return;
                    }
                }
                Ok(Some(ServerEvent::ClientInit { .. })) => {
                    let init = ServerInit { width: 64, height: 64, pixel_format: PixelFormat::RGB888, name: "f".into() };
                    if server.send_server_init(&init).is_err() {
                        return;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => return,
            }
        }
        let _ = server.take_output();
    }
});
