//! M1 acceptance tests (Q2): a Farsight client and server state machine talk
//! to each other in memory, across protocol versions and legacy security types.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use farsight_codec::{Decoder, Framebuffer, PixelConverter, encode};
use farsight_rfb::auth::{Authenticator, NoAuth};
use farsight_rfb::client::{ClientConfig, ClientConnection, ClientEvent};
use farsight_rfb::handshake::ServerInit;
use farsight_rfb::messages::RECTS_UNTIL_LAST_RECT;
use farsight_rfb::server::{ServerConfig, ServerConnection, ServerEvent};
use farsight_rfb::{
    ClientMessage, CutText, Encoding, Limits, PixelFormat, ProtocolError, ProtocolVersion, Rect,
    RectHeader, SecurityType, ServerMessage,
};
use farsight_security::SecurityPolicy;
use farsight_security::vnc_auth::{VncAuthClient, VncAuthServer};
use secrecy::SecretString;

const PASSWORD: &str = "s3cret";

struct Harness {
    client: ClientConnection,
    server: ServerConnection,
    decoder: Decoder,
    client_password: &'static str,
    client_events: Vec<ClientEvent>,
    server_events: Vec<ServerEvent>,
}

impl Harness {
    fn new(
        client_version: ProtocolVersion,
        server_types: Vec<SecurityType>,
        client_password: &'static str,
    ) -> Self {
        Self {
            client: ClientConnection::new(ClientConfig {
                max_version: client_version,
                ..ClientConfig::default()
            }),
            server: ServerConnection::new(ServerConfig {
                max_version: ProtocolVersion::V3_8,
                security_types: server_types,
                limits: Limits::DEFAULT,
            }),
            decoder: Decoder::new(0, 0, PixelFormat::RGB888, Limits::DEFAULT),
            client_password,
            client_events: Vec::new(),
            server_events: Vec::new(),
        }
    }

    /// Shuttles bytes until neither side makes progress.
    fn run(&mut self) -> Result<(), ProtocolError> {
        loop {
            let mut progressed = false;
            let to_client = self.server.take_output();
            if !to_client.is_empty() {
                progressed = true;
                // Deliver one byte at a time to exercise incremental parsing.
                for byte in to_client {
                    self.client.receive(&[byte]);
                    self.poll_client()?;
                }
            }
            let to_server = self.client.take_output();
            if !to_server.is_empty() {
                progressed = true;
                self.server.receive(&to_server);
            }
            progressed |= self.poll_server()?;
            if !progressed {
                return Ok(());
            }
        }
    }

    fn poll_client(&mut self) -> Result<(), ProtocolError> {
        while let Some(event) = self.client.poll(&mut self.decoder)? {
            match &event {
                ClientEvent::SecurityTypes(offered) => {
                    let policy = SecurityPolicy::LOOPBACK;
                    let chosen = policy.select(offered).expect("acceptable security type");
                    let auth: Box<dyn Authenticator> = match chosen {
                        SecurityType::VncAuth => Box::new(VncAuthClient::new(SecretString::from(
                            self.client_password.to_owned(),
                        ))),
                        _ => Box::new(NoAuth),
                    };
                    self.client.choose_security(chosen, auth)?;
                }
                ClientEvent::Connected(init) => {
                    self.decoder =
                        Decoder::new(init.width, init.height, init.pixel_format, Limits::DEFAULT);
                }
                _ => {}
            }
            self.client_events.push(event);
        }
        Ok(())
    }

    fn poll_server(&mut self) -> Result<bool, ProtocolError> {
        let mut progressed = false;
        while let Some(event) = self.server.poll()? {
            progressed = true;
            match &event {
                ServerEvent::SecuritySelected(security) => {
                    let auth: Box<dyn Authenticator> = match security {
                        SecurityType::VncAuth => Box::new(VncAuthServer::new(
                            SecretString::from(PASSWORD.to_owned()),
                            [0x5a; 16],
                        )),
                        _ => Box::new(NoAuth),
                    };
                    self.server.start_authentication(*security, auth)?;
                }
                ServerEvent::ClientInit { .. } => {
                    self.server.send_server_init(&ServerInit {
                        width: 8,
                        height: 4,
                        pixel_format: PixelFormat::RGB888,
                        name: "test desktop".into(),
                    })?;
                }
                _ => {}
            }
            self.server_events.push(event);
        }
        Ok(progressed)
    }
}

fn connected_desktop_name(h: &Harness) -> Option<&str> {
    h.client_events.iter().find_map(|e| match e {
        ClientEvent::Connected(init) => Some(init.name.as_str()),
        _ => None,
    })
}

#[test]
fn handshake_succeeds_for_every_version_and_legacy_type() {
    for version in [
        ProtocolVersion::V3_3,
        ProtocolVersion::V3_7,
        ProtocolVersion::V3_8,
    ] {
        for security in [SecurityType::None, SecurityType::VncAuth] {
            let mut h = Harness::new(version, vec![security], PASSWORD);
            h.run()
                .unwrap_or_else(|e| panic!("{version:?}/{security:?}: {e}"));
            assert!(h.client.is_connected(), "{version:?}/{security:?}");
            assert!(h.server.is_connected(), "{version:?}/{security:?}");
            assert_eq!(h.client.version(), version);
            assert_eq!(connected_desktop_name(&h), Some("test desktop"));
            assert!(
                h.server_events
                    .contains(&ServerEvent::ClientInit { shared: true })
            );
        }
    }
}

#[test]
fn wrong_password_is_refused_with_a_reason_in_3_8() {
    let mut h = Harness::new(ProtocolVersion::V3_8, vec![SecurityType::VncAuth], "wrong");
    let server_error = h.run().unwrap_err();
    assert!(matches!(server_error, ProtocolError::Auth(_)));
    // The failure reason is still flushed to the client.
    for byte in h.server.take_output() {
        h.client.receive(&[byte]);
    }
    assert_eq!(
        h.client.poll(&mut h.decoder),
        Err(ProtocolError::Refused("authentication failed".into()))
    );
    assert!(!h.client.is_connected());
}

#[test]
fn wrong_password_is_refused_without_reason_in_3_7() {
    let mut h = Harness::new(ProtocolVersion::V3_7, vec![SecurityType::VncAuth], "wrong");
    assert!(h.run().is_err());
    h.client.receive(&h.server.take_output());
    assert!(matches!(
        h.client.poll(&mut h.decoder),
        Err(ProtocolError::Refused(_))
    ));
}

#[test]
fn rfb_3_3_client_is_refused_when_only_modern_security_is_offered() {
    let mut h = Harness::new(
        ProtocolVersion::V3_3,
        vec![SecurityType::VeNCrypt],
        PASSWORD,
    );
    assert!(matches!(h.run(), Err(ProtocolError::Refused(_))));
}

#[test]
fn input_sent_before_authentication_is_never_parsed_as_input() {
    let mut server = ServerConnection::new(ServerConfig {
        max_version: ProtocolVersion::V3_8,
        security_types: vec![SecurityType::VncAuth],
        limits: Limits::DEFAULT,
    });
    server.receive(&ProtocolVersion::V3_8.to_bytes());
    server.receive(&[SecurityType::VncAuth.code()]);
    let mut key = Vec::new();
    ClientMessage::KeyEvent {
        down: true,
        keysym: 0x61,
    }
    .encode(&mut key)
    .unwrap();
    key.resize(16, 0); // padded to look like a challenge response
    server.receive(&key);
    assert_eq!(
        server.poll(),
        Ok(Some(ServerEvent::SecuritySelected(SecurityType::VncAuth)))
    );
    server
        .start_authentication(
            SecurityType::VncAuth,
            Box::new(VncAuthServer::new(
                SecretString::from(PASSWORD.to_owned()),
                [1; 16],
            )),
        )
        .unwrap();
    assert!(matches!(server.poll(), Err(ProtocolError::Auth(_))));
    assert_eq!(server.poll(), Ok(None));
    assert!(!server.is_connected());
}

#[test]
fn sending_before_the_handshake_is_an_error() {
    let mut client = ClientConnection::new(ClientConfig::default());
    assert!(matches!(
        client.send(&ClientMessage::KeyEvent {
            down: true,
            keysym: 1
        }),
        Err(ProtocolError::InvalidState(_))
    ));
}

fn connected() -> Harness {
    let mut h = Harness::new(ProtocolVersion::V3_8, vec![SecurityType::None], PASSWORD);
    h.run().unwrap();
    h.client_events.clear();
    h.server_events.clear();
    h
}

#[test]
fn framebuffer_updates_with_raw_copyrect_and_last_rect() {
    let mut h = connected();
    let mut screen = Framebuffer::new(8, 4);
    screen
        .write(Rect::new(0, 0, 8, 4), |i| {
            u32::try_from(i).unwrap() * 0x0001_0203
        })
        .unwrap();
    let converter = PixelConverter::new(PixelFormat::RGB888);

    h.client
        .send(&ClientMessage::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 8, 4),
        })
        .unwrap();
    h.run().unwrap();
    assert!(matches!(
        h.server_events.as_slice(),
        [ServerEvent::Message(
            ClientMessage::FramebufferUpdateRequest {
                incremental: false,
                ..
            }
        )]
    ));

    let whole = Rect::new(0, 0, 8, 4);
    h.server
        .send(&ServerMessage::FramebufferUpdate {
            rects: RECTS_UNTIL_LAST_RECT,
        })
        .unwrap();
    h.server
        .send_rect(
            &RectHeader {
                rect: whole,
                encoding: Encoding::Raw,
            },
            &encode::raw(&screen, whole, &converter).unwrap(),
        )
        .unwrap();
    h.server
        .send_rect(
            &RectHeader {
                rect: Rect::new(4, 2, 4, 2),
                encoding: Encoding::CopyRect,
            },
            &encode::copy_rect(0, 0),
        )
        .unwrap();
    h.server
        .send_rect(
            &RectHeader {
                rect: Rect::default(),
                encoding: Encoding::LastRect,
            },
            &[],
        )
        .unwrap();
    h.run().unwrap();

    screen.copy(0, 0, Rect::new(4, 2, 4, 2)).unwrap();
    assert_eq!(h.decoder.framebuffer(), &screen);
    assert_eq!(
        h.client_events
            .iter()
            .filter(|e| matches!(e, ClientEvent::Rect(_)))
            .count(),
        2
    );
    assert_eq!(h.client_events.last(), Some(&ClientEvent::UpdateComplete));
}

#[test]
fn desktop_resize_pseudo_rect_resizes_the_framebuffer() {
    let mut h = connected();
    h.server
        .send(&ServerMessage::FramebufferUpdate { rects: 1 })
        .unwrap();
    h.server
        .send_rect(
            &RectHeader {
                rect: Rect::new(0, 0, 16, 9),
                encoding: Encoding::DesktopSize,
            },
            &[],
        )
        .unwrap();
    h.run().unwrap();
    assert_eq!(
        (
            h.decoder.framebuffer().width(),
            h.decoder.framebuffer().height()
        ),
        (16, 9)
    );
}

#[test]
fn input_and_clipboard_reach_the_server() {
    let mut h = connected();
    let messages = [
        ClientMessage::SetEncodings(vec![Encoding::CopyRect, Encoding::Raw, Encoding::LastRect]),
        ClientMessage::KeyEvent {
            down: true,
            keysym: 0xff0d,
        },
        ClientMessage::PointerEvent {
            buttons: 1,
            x: 3,
            y: 2,
        },
        ClientMessage::ClientCutText(CutText::Text(b"copied".to_vec())),
    ];
    for m in &messages {
        h.client.send(m).unwrap();
    }
    h.run().unwrap();
    let received: Vec<_> = h
        .server_events
        .iter()
        .filter_map(|e| match e {
            ServerEvent::Message(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(received, messages);
}

#[test]
fn oversized_server_clipboard_is_skipped_and_the_session_continues() {
    let mut h = connected();
    let big = vec![b'x'; usize::try_from(Limits::DEFAULT.max_cut_text).unwrap() + 1];
    h.server
        .send(&ServerMessage::ServerCutText(CutText::Text(big)))
        .unwrap();
    h.server.send(&ServerMessage::Bell).unwrap();
    // Deliver in large chunks: byte-at-a-time would be slow for 16 MiB.
    let bytes = h.server.take_output();
    for chunk in bytes.chunks(1 << 20) {
        h.client.receive(chunk);
        while let Some(event) = h.client.poll(&mut h.decoder).unwrap() {
            h.client_events.push(event);
        }
    }
    assert!(matches!(
        h.client_events.first(),
        Some(ClientEvent::CutTextDiscarded { .. })
    ));
    assert_eq!(h.client_events.last(), Some(&ClientEvent::Bell));
}

/// A hand-assembled RFB 3.8 server transcript (RFC 6143 §7.1–7.3), checked
/// byte-for-byte against what the client sends back.
#[test]
fn rfc_6143_transcript_none_security() {
    let mut client = ClientConnection::new(ClientConfig::default());
    let mut decoder = Decoder::new(0, 0, PixelFormat::RGB888, Limits::DEFAULT);
    client.receive(b"RFB 003.008\n");
    client.receive(&[1, 1]); // one security type: None
    assert_eq!(
        client.poll(&mut decoder),
        Ok(Some(ClientEvent::SecurityTypes(vec![SecurityType::None])))
    );
    assert_eq!(client.take_output(), b"RFB 003.008\n");
    client
        .choose_security(SecurityType::None, Box::new(NoAuth))
        .unwrap();
    client.receive(&[0, 0, 0, 0]); // SecurityResult OK
    client.receive(&[0, 2, 0, 1]); // 2x1
    client.receive(&PixelFormat::RGB888.encode());
    client.receive(&[0, 0, 0, 1, b'x']);
    let Ok(Some(ClientEvent::Connected(init))) = client.poll(&mut decoder) else {
        panic!("expected Connected");
    };
    assert_eq!((init.width, init.height, init.name.as_str()), (2, 1, "x"));
    // Selected type (1), then ClientInit shared=1.
    assert_eq!(client.take_output(), [1, 1]);
}
