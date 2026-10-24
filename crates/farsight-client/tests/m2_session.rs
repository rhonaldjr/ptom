//! M2 acceptance tests (Q2) for the viewer session use case, against a real
//! `ServerConnection` acting as the server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use farsight_client::{Session, SessionConfig, SessionEvent, SessionStatus};
use farsight_rfb::auth::{Authenticator, NoAuth};
use farsight_rfb::handshake::ServerInit;
use farsight_rfb::server::{ServerConfig, ServerConnection, ServerEvent};
use farsight_rfb::{
    ClientMessage, Limits, PixelFormat, ProtocolVersion, Rect, SecurityType, ServerMessage,
};
use farsight_security::vnc_auth::VncAuthServer;
use secrecy::SecretString;

struct World {
    session: Session,
    server: ServerConnection,
    events: Vec<SessionEvent>,
    received: Vec<ClientMessage>,
}

impl World {
    fn new(types: Vec<SecurityType>, config: SessionConfig) -> Self {
        Self {
            session: Session::new(config),
            server: ServerConnection::new(ServerConfig {
                max_version: ProtocolVersion::V3_8,
                security_types: types,
                limits: Limits::DEFAULT,
            }),
            events: Vec::new(),
            received: Vec::new(),
        }
    }

    fn run(&mut self) {
        for _ in 0..50 {
            self.session.receive(&self.server.take_output());
            self.events.extend(self.session.poll().unwrap());
            self.server.receive(&self.session.take_output());
            while let Some(event) = self.server.poll().unwrap() {
                match event {
                    ServerEvent::SecuritySelected(t) => {
                        let auth: Box<dyn Authenticator> = if t == SecurityType::VncAuth {
                            Box::new(VncAuthServer::new(
                                SecretString::from("pw".to_owned()),
                                [9; 16],
                            ))
                        } else {
                            Box::new(NoAuth)
                        };
                        self.server.start_authentication(t, auth).unwrap();
                    }
                    ServerEvent::ClientInit { .. } => self
                        .server
                        .send_server_init(&ServerInit {
                            width: 32,
                            height: 16,
                            pixel_format: PixelFormat::RGB888,
                            name: "desk".into(),
                        })
                        .unwrap(),
                    ServerEvent::Message(m) => self.received.push(m),
                    ServerEvent::UpgradeTransport(_) | ServerEvent::CutTextDiscarded { .. } => {}
                }
            }
        }
    }
}

fn network() -> SessionConfig {
    SessionConfig {
        shared: true,
        ..SessionConfig::default()
    }
}

#[test]
fn unencrypted_server_on_the_network_requires_consent_then_password() {
    let mut w = World::new(vec![SecurityType::VncAuth], network());
    w.run();
    assert_eq!(
        w.events,
        [SessionEvent::InsecureConsentRequired {
            security: SecurityType::VncAuth
        }]
    );
    assert_eq!(w.session.status(), SessionStatus::WaitingForInsecureConsent);

    assert_eq!(
        w.session.consent_insecure().unwrap(),
        Some(SessionEvent::CredentialsRequired { username: false })
    );
    assert_eq!(w.session.status(), SessionStatus::WaitingForCredentials);
    w.session
        .provide_password(SecretString::from("pw".to_owned()))
        .unwrap();
    w.run();
    assert_eq!(w.session.status(), SessionStatus::Connected);
    assert!(w.events.iter().any(|e| matches!(
        e,
        SessionEvent::Connected {
            encrypted: false,
            security: SecurityType::VncAuth,
            ..
        }
    )));
}

#[test]
fn loopback_connects_without_prompts() {
    let mut w = World::new(
        vec![SecurityType::None],
        SessionConfig {
            loopback: true,
            ..network()
        },
    );
    w.run();
    assert_eq!(w.session.status(), SessionStatus::Connected);
    assert!(matches!(
        w.events.first(),
        Some(SessionEvent::Connected {
            width: 32,
            height: 16,
            ..
        })
    ));
}

#[test]
fn first_messages_set_format_encodings_and_request_a_full_update() {
    let mut w = World::new(
        vec![SecurityType::None],
        SessionConfig {
            loopback: true,
            ..network()
        },
    );
    w.run();
    assert!(
        matches!(w.received.first(), Some(ClientMessage::SetPixelFormat(f)) if *f == PixelFormat::RGB888)
    );
    assert!(matches!(
        w.received.get(1),
        Some(ClientMessage::SetEncodings(_))
    ));
    assert_eq!(
        w.received.get(2),
        Some(&ClientMessage::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 32, 16)
        })
    );
}

#[test]
fn incremental_update_follows_each_completed_update() {
    let mut w = World::new(
        vec![SecurityType::None],
        SessionConfig {
            loopback: true,
            ..network()
        },
    );
    w.run();
    w.received.clear();
    w.server
        .send(&ServerMessage::FramebufferUpdate { rects: 0 })
        .unwrap();
    w.run();
    assert!(w.events.contains(&SessionEvent::UpdateComplete));
    assert_eq!(
        w.received,
        [ClientMessage::FramebufferUpdateRequest {
            incremental: true,
            rect: Rect::new(0, 0, 32, 16)
        }]
    );
}

#[test]
fn view_only_suppresses_all_input() {
    let config = SessionConfig {
        loopback: true,
        view_only: true,
        ..network()
    };
    let mut w = World::new(vec![SecurityType::None], config);
    w.run();
    w.received.clear();
    w.session.key(true, 0x61);
    w.session.pointer(1, 5, 5);
    w.session.clipboard("secret");
    w.run();
    assert_eq!(w.received, Vec::<ClientMessage>::new());
}

#[test]
fn input_is_clamped_and_clipboard_deduplicated() {
    let mut w = World::new(
        vec![SecurityType::None],
        SessionConfig {
            loopback: true,
            ..network()
        },
    );
    w.run();
    w.received.clear();
    w.session.pointer(0, 999, 999);
    w.session.clipboard("hello");
    w.session.clipboard("hello");
    w.run();
    assert_eq!(w.received.len(), 2);
    assert_eq!(
        w.received.first(),
        Some(&ClientMessage::PointerEvent {
            buttons: 0,
            x: 31,
            y: 15
        })
    );
}

#[test]
fn server_clipboard_is_delivered_and_not_echoed_back() {
    let mut w = World::new(
        vec![SecurityType::None],
        SessionConfig {
            loopback: true,
            ..network()
        },
    );
    w.run();
    w.received.clear();
    w.server
        .send(&ServerMessage::ServerCutText(farsight_rfb::CutText::Text(
            b"from server".to_vec(),
        )))
        .unwrap();
    w.run();
    assert!(
        w.events
            .contains(&SessionEvent::Clipboard("from server".into()))
    );
    w.session.clipboard("from server");
    w.run();
    assert_eq!(w.received, Vec::<ClientMessage>::new());
}

#[test]
fn server_without_supported_security_is_refused() {
    let mut session = Session::new(network());
    let mut server = ServerConnection::new(ServerConfig {
        max_version: ProtocolVersion::V3_8,
        security_types: vec![SecurityType::Tight],
        limits: Limits::DEFAULT,
    });
    session.receive(&server.take_output());
    session.poll().unwrap();
    server.receive(&session.take_output());
    server.poll().unwrap();
    session.receive(&server.take_output());
    assert!(session.poll().is_err());
    assert_eq!(session.status(), SessionStatus::Closed);
}

#[test]
fn resize_requests_need_server_support() {
    let mut w = World::new(
        vec![SecurityType::None],
        SessionConfig {
            loopback: true,
            ..network()
        },
    );
    w.run();
    w.received.clear();
    // No ExtendedDesktopSize rectangle yet, so the server is not known to support it.
    assert!(!w.session.request_resize(800, 600));
    let mut payload = Vec::new();
    payload.extend_from_slice(&[1, 0, 0, 0]);
    farsight_rfb::Screen {
        id: 7,
        x: 0,
        y: 0,
        width: 32,
        height: 16,
        flags: 0,
    }
    .encode(&mut payload);
    w.server
        .send(&ServerMessage::FramebufferUpdate { rects: 1 })
        .unwrap();
    w.server
        .send_rect(
            &farsight_rfb::RectHeader {
                rect: Rect::new(0, 0, 32, 16),
                encoding: farsight_rfb::Encoding::ExtendedDesktopSize,
            },
            &payload,
        )
        .unwrap();
    w.run();
    w.received.clear();
    assert!(w.session.request_resize(800, 600));
    assert!(
        !w.session.request_resize(32, 16),
        "same size is not re-requested"
    );
    w.run();
    assert!(matches!(
        w.received.first(),
        Some(ClientMessage::SetDesktopSize { width: 800, height: 600, screens }) if screens.first().map(|s| s.id) == Some(7)
    ));
}
