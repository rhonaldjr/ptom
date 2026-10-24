#![no_main]
//! A malicious server: arbitrary bytes into the viewer's state machine.

use farsight_codec::Decoder;
use farsight_rfb::auth::{Authenticator, NoAuth};
use farsight_rfb::client::{ClientConfig, ClientConnection, ClientEvent};
use farsight_rfb::{Limits, PixelFormat, SecurityType};
use farsight_security::vnc_auth::VncAuthClient;
use libfuzzer_sys::fuzz_target;
use secrecy::SecretString;

fuzz_target!(|data: &[u8]| {
    let mut client = ClientConnection::new(ClientConfig::default());
    let mut decoder = Decoder::new(0, 0, PixelFormat::RGB888, Limits::DEFAULT);
    for chunk in data.chunks(7) {
        client.receive(chunk);
        loop {
            match client.poll(&mut decoder) {
                Ok(Some(ClientEvent::SecurityTypes(offered))) => {
                    let Some(&chosen) = offered.first() else { return };
                    let auth: Box<dyn Authenticator> = if chosen == SecurityType::VncAuth {
                        Box::new(VncAuthClient::new(SecretString::from("pw".to_owned())))
                    } else {
                        Box::new(NoAuth)
                    };
                    if client.choose_security(chosen, auth).is_err() {
                        return;
                    }
                }
                Ok(Some(ClientEvent::Connected(init))) => {
                    decoder = Decoder::new(init.width, init.height, init.pixel_format, Limits::DEFAULT);
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => return,
            }
        }
        let _ = client.take_output();
    }
});
