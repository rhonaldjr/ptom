//! M3 acceptance tests (Q2): encrypted security types, identity pinning,
//! credential prompts and downgrade protection, against real server-side
//! authenticators. TLS itself is simulated (the transport adapter's job);
//! RA2 framing is real.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use farsight_client::{IdentityKind, Session, SessionConfig, SessionEvent, SessionStatus};
use farsight_rfb::auth::{Authenticator, NoAuth, TransportUpgrade};
use farsight_rfb::handshake::ServerInit;
use farsight_rfb::server::{ServerConfig, ServerConnection, ServerEvent};
use farsight_rfb::{Limits, PixelFormat, ProtocolError, ProtocolVersion, SecurityType};
use farsight_security::ra2::{Ra2Cipher, Ra2Credentials, Ra2Server, Ra2ServerKey, Ra2Variant};
use farsight_security::vencrypt::{PlainVerifier, VeNCryptServer};
use farsight_security::vnc_auth::VncAuthServer;
use secrecy::{ExposeSecret, SecretString};

const CERT_FINGERPRINT: [u8; 32] = [0xab; 32];
const DESKTOP_NAME: &str = "secret-desktop-name";

struct Alice;
impl PlainVerifier for Alice {
    fn verify(&self, username: &str, password: &SecretString) -> bool {
        username == "alice" && password.expose_secret() == "wonderland"
    }
}

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

#[derive(Clone, Copy)]
enum Kind {
    VeNCrypt,
    Ra2(Ra2Variant),
    VncAuthOnly,
}

struct World {
    session: Session,
    server: ServerConnection,
    kind: Kind,
    ra2_key: Arc<Ra2ServerKey>,
    client_cipher: Option<Ra2Cipher>,
    server_cipher: Option<Ra2Cipher>,
    /// Raw bytes that crossed the "wire" after the handshake began.
    wire: Vec<u8>,
    events: Vec<SessionEvent>,
}

impl World {
    fn new(kind: Kind, config: SessionConfig) -> Self {
        Self::with_key(kind, config, Arc::new(Ra2ServerKey::generate().unwrap()))
    }

    fn with_key(kind: Kind, config: SessionConfig, ra2_key: Arc<Ra2ServerKey>) -> Self {
        let security = match kind {
            Kind::VeNCrypt => SecurityType::VeNCrypt,
            Kind::Ra2(Ra2Variant::Sha1) => SecurityType::Ra2,
            Kind::Ra2(Ra2Variant::Sha256) => SecurityType::Ra256,
            Kind::VncAuthOnly => SecurityType::VncAuth,
        };
        Self {
            session: Session::new(config),
            server: ServerConnection::new(ServerConfig {
                max_version: ProtocolVersion::V3_8,
                security_types: vec![security],
                limits: Limits::DEFAULT,
            }),
            kind,
            ra2_key,
            client_cipher: None,
            server_cipher: None,
            wire: Vec::new(),
            events: Vec::new(),
        }
    }

    fn server_auth(&self, security: SecurityType) -> Box<dyn Authenticator> {
        match self.kind {
            Kind::VeNCrypt => Box::new(VeNCryptServer::new(
                Some(Box::new(Alice)),
                None,
                [0; 16],
                false,
            )),
            Kind::Ra2(variant) => Box::new(Ra2Server::new(
                variant,
                Arc::clone(&self.ra2_key),
                Ra2Credentials::UserPassword(Box::new(Alice)),
            )),
            Kind::VncAuthOnly if security == SecurityType::VncAuth => {
                Box::new(VncAuthServer::new(secret("pw"), [1; 16]))
            }
            Kind::VncAuthOnly => Box::new(NoAuth),
        }
    }

    /// Moves bytes until nothing changes. Returns the first error from either side.
    fn run(&mut self) -> Result<(), ProtocolError> {
        for _ in 0..100 {
            // Server → client.
            let out = self.server.take_output();
            let raw = seal(self.server_cipher.as_mut(), out);
            self.wire.extend(&raw);
            let plain = open(self.client_cipher.as_mut(), &raw);
            self.session.receive(&plain);
            for event in self.session.poll()? {
                self.on_session_event(event)?;
            }
            // Client → server.
            let out = self.session.take_output();
            let raw = seal(self.client_cipher.as_mut(), out);
            self.wire.extend(&raw);
            let plain = open(self.server_cipher.as_mut(), &raw);
            self.server.receive(&plain);
            while let Some(event) = self.server.poll()? {
                match event {
                    ServerEvent::SecuritySelected(t) => {
                        let auth = self.server_auth(t);
                        self.server.start_authentication(t, auth)?;
                    }
                    ServerEvent::UpgradeTransport(TransportUpgrade::Ra2 {
                        send_key,
                        receive_key,
                    }) => {
                        self.wire.extend(self.server.take_output());
                        self.server_cipher = Some(Ra2Cipher::new(&send_key, &receive_key).unwrap());
                        self.server.transport_upgraded()?;
                    }
                    ServerEvent::UpgradeTransport(_) => self.server.transport_upgraded()?,
                    ServerEvent::ClientInit { .. } => {
                        self.server.send_server_init(&ServerInit {
                            width: 16,
                            height: 8,
                            pixel_format: PixelFormat::RGB888,
                            name: DESKTOP_NAME.into(),
                        })?;
                    }
                    ServerEvent::Message(_) | ServerEvent::CutTextDiscarded { .. } => {}
                }
            }
        }
        Ok(())
    }

    fn on_session_event(&mut self, event: SessionEvent) -> Result<(), ProtocolError> {
        match &event {
            // What farsight-net does: TLS for VeNCrypt, AES-EAX framing for RA2.
            SessionEvent::TransportUpgrade(TransportUpgrade::Tls) => {
                if let Some(e) = self.session.tls_established(CERT_FINGERPRINT, false)? {
                    self.events.push(e);
                }
            }
            SessionEvent::TransportUpgrade(TransportUpgrade::Ra2 {
                send_key,
                receive_key,
            }) => {
                // Bytes queued before the upgrade still go out in plaintext.
                let pending = self.session.take_output();
                self.wire.extend(&pending);
                self.server.receive(&pending);
                self.client_cipher = Some(Ra2Cipher::new(send_key, receive_key).unwrap());
                self.session.transport_upgraded()?;
            }
            _ => {}
        }
        self.events.push(event);
        Ok(())
    }

    fn identity_prompts(&self) -> Vec<(IdentityKind, bool)> {
        self.events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::IdentityConfirmationRequired { kind, changed, .. } => {
                    Some((*kind, *changed))
                }
                _ => None,
            })
            .collect()
    }
}

fn seal(cipher: Option<&mut Ra2Cipher>, plain: Vec<u8>) -> Vec<u8> {
    match cipher {
        Some(c) if !plain.is_empty() => {
            let mut out = Vec::new();
            c.seal(&plain, &mut out).unwrap();
            out
        }
        _ => plain,
    }
}

fn open(cipher: Option<&mut Ra2Cipher>, raw: &[u8]) -> Vec<u8> {
    let Some(c) = cipher else { return raw.to_vec() };
    let mut plain = Vec::new();
    let mut rest = raw;
    while let Some((bytes, used)) = c.open(rest).unwrap() {
        plain.extend(bytes);
        rest = &rest[used..];
    }
    assert!(rest.is_empty(), "frames are never split in this harness");
    plain
}

fn network() -> SessionConfig {
    SessionConfig {
        shared: true,
        ..SessionConfig::default()
    }
}

fn connect_with_prompts(w: &mut World) {
    w.run().unwrap();
    if w.session.status() == SessionStatus::WaitingForIdentity {
        w.session.confirm_identity().unwrap();
        w.run().unwrap();
    }
    if w.session.status() == SessionStatus::WaitingForCredentials {
        w.session
            .provide_credentials(Some(secret("alice")), secret("wonderland"))
            .unwrap();
        w.run().unwrap();
    }
}

#[test]
fn vencrypt_x509_plain_asks_identity_then_credentials_then_connects_encrypted() {
    let mut w = World::new(Kind::VeNCrypt, network());
    w.run().unwrap();
    assert_eq!(w.session.status(), SessionStatus::WaitingForIdentity);
    assert_eq!(w.identity_prompts(), [(IdentityKind::Certificate, false)]);
    w.session.confirm_identity().unwrap();
    w.run().unwrap();
    assert!(
        w.events
            .contains(&SessionEvent::CredentialsRequired { username: true })
    );
    w.session
        .provide_credentials(Some(secret("alice")), secret("wonderland"))
        .unwrap();
    w.run().unwrap();
    assert_eq!(w.session.status(), SessionStatus::Connected);
    assert!(w.session.encrypted());
    assert_eq!(
        w.session.presented_identity(),
        Some((IdentityKind::Certificate, CERT_FINGERPRINT))
    );
    assert!(w.events.iter().any(|e| matches!(
        e,
        SessionEvent::Connected {
            encrypted: true,
            ..
        }
    )));
}

/// Regression (TigerVNC 1.15 interop): with TLS 1.3 the server sends its
/// VncAuth challenge as soon as TLS is up, while the user is still checking
/// the certificate. Those bytes came through TLS and must be accepted once
/// the identity is confirmed, not rejected as plaintext injection.
#[test]
fn server_data_that_arrives_during_the_identity_check_is_kept() {
    let mut session = Session::new(SessionConfig {
        password: Some(secret("pw")),
        ..network()
    });
    let mut events = Vec::new();
    // What TigerVNC sends before TLS: version, VeNCrypt, its version 0.2,
    // "ok" plus one subtype (X509Vnc = 261), then "subtype accepted".
    let mut feed = |session: &mut Session, bytes: &[u8]| {
        session.receive(bytes);
        events.extend(session.poll().unwrap());
        session.take_output();
    };
    feed(&mut session, b"RFB 003.008\n");
    feed(&mut session, &[1, 19]);
    feed(&mut session, &[0, 2]);
    feed(&mut session, &[0, 1, 0, 0, 1, 5]);
    feed(&mut session, &[1]);
    assert!(events.contains(&SessionEvent::TransportUpgrade(TransportUpgrade::Tls)));
    assert!(
        session
            .tls_established(CERT_FINGERPRINT, false)
            .unwrap()
            .is_some()
    );
    // The challenge arrives (over TLS) before the user decides.
    session.receive(&[9; 16]);
    session.confirm_identity().unwrap();
    session.poll().unwrap();
    assert_eq!(
        session.take_output().len(),
        16,
        "the VncAuth response is sent"
    );
}

#[test]
fn pinned_certificate_skips_the_identity_prompt() {
    let config = SessionConfig {
        pinned_identity: Some(CERT_FINGERPRINT),
        username: Some(secret("alice")),
        password: Some(secret("wonderland")),
        ..network()
    };
    let mut w = World::new(Kind::VeNCrypt, config);
    w.run().unwrap();
    assert_eq!(w.session.status(), SessionStatus::Connected);
    assert_eq!(w.identity_prompts(), []);
}

#[test]
fn changed_certificate_is_flagged() {
    let config = SessionConfig {
        pinned_identity: Some([0x11; 32]),
        ..network()
    };
    let mut w = World::new(Kind::VeNCrypt, config);
    w.run().unwrap();
    assert_eq!(w.identity_prompts(), [(IdentityKind::Certificate, true)]);
    assert_eq!(w.session.status(), SessionStatus::WaitingForIdentity);
}

#[test]
fn ra2_and_ra2_256_connect_with_encrypted_traffic() {
    for variant in [Ra2Variant::Sha1, Ra2Variant::Sha256] {
        let mut w = World::new(Kind::Ra2(variant), network());
        connect_with_prompts(&mut w);
        assert_eq!(w.session.status(), SessionStatus::Connected, "{variant:?}");
        assert!(w.session.encrypted());
        assert_eq!(w.identity_prompts(), [(IdentityKind::RsaKey, false)]);
        // The desktop name only ever crossed the wire encrypted.
        let wire = String::from_utf8_lossy(&w.wire).into_owned();
        assert!(!wire.contains(DESKTOP_NAME), "{variant:?} leaked plaintext");
        assert!(
            !wire.contains("wonderland"),
            "{variant:?} leaked the password"
        );
    }
}

#[test]
fn pinned_ra2_key_skips_the_prompt_and_a_new_key_is_flagged() {
    let key = Arc::new(Ra2ServerKey::generate().unwrap());
    let mut first = World::with_key(Kind::Ra2(Ra2Variant::Sha256), network(), Arc::clone(&key));
    connect_with_prompts(&mut first);
    let (_, pinned) = first.session.presented_identity().unwrap();
    assert_eq!(pinned, key.fingerprint());

    let config = SessionConfig {
        pinned_identity: Some(pinned),
        username: Some(secret("alice")),
        password: Some(secret("wonderland")),
        ..network()
    };
    let mut second = World::with_key(Kind::Ra2(Ra2Variant::Sha256), config.clone(), key);
    second.run().unwrap();
    assert_eq!(second.session.status(), SessionStatus::Connected);
    assert_eq!(second.identity_prompts(), []);

    // A different server key (e.g. a man in the middle) is flagged as changed.
    let mut impostor = World::new(Kind::Ra2(Ra2Variant::Sha256), config);
    impostor.run().unwrap();
    assert_eq!(impostor.identity_prompts(), [(IdentityKind::RsaKey, true)]);
    assert_eq!(impostor.session.status(), SessionStatus::WaitingForIdentity);
}

#[test]
fn downgrade_to_unencrypted_security_is_refused() {
    let config = SessionConfig {
        expect_encryption: true,
        allow_insecure: true,
        ..network()
    };
    let mut w = World::new(Kind::VncAuthOnly, config);
    let error = w.run().unwrap_err();
    assert!(
        matches!(&error, ProtocolError::Refused(reason) if reason.contains("interfering")),
        "{error}"
    );
    assert_eq!(w.session.status(), SessionStatus::Closed);
}

#[test]
fn secrets_never_appear_in_debug_output() {
    let config = SessionConfig {
        username: Some(secret("alice")),
        password: Some(secret("wonderland")),
        ..network()
    };
    let rendered = format!("{config:?}");
    assert!(!rendered.contains("wonderland"), "{rendered}");
    let mut w = World::new(Kind::Ra2(Ra2Variant::Sha1), config);
    connect_with_prompts(&mut w);
    assert_eq!(w.session.status(), SessionStatus::Connected);
    let events = format!("{:?}", w.events);
    assert!(!events.contains("wonderland"));
    // Session keys are redacted in TransportUpgrade's Debug output.
    assert!(events.contains("Ra2 { .. }"));
}
