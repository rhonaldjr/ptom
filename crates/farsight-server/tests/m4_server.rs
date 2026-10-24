//! M4 acceptance tests (Q2): the viewer's `farsight_client::Session` against
//! the agent's `ServerSession`, in memory. TLS is simulated (the transport
//! adapter's job); RA2 framing is real.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use farsight_client::{Session, SessionConfig, SessionEvent as ViewerEvent, SessionStatus};
use farsight_codec::{Cursor, Decoder, Framebuffer, PixelConverter};
use farsight_codec::{ResizeReason, ResizeStatus};
use farsight_rfb::auth::{NoAuth, TransportUpgrade};
use farsight_rfb::client::{ClientConfig, ClientConnection, ClientEvent};
use farsight_rfb::{
    ClientMessage, Encoding, Limits, PixelFormat, ProtocolError, Rect, Screen, SecurityType,
};
use farsight_security::ra2::{Ra2Cipher, Ra2ServerKey, Ra2Variant};
use farsight_server::{
    AccessControl, AccessPolicy, Admission, ClipboardSink, Desktop, DesktopResizer, Entropy,
    InputSink, Refusal, ResizeOutcome, ServerSession, ServerSessionConfig, SessionEvent,
    SessionPorts, UserVerifier,
};
use secrecy::{ExposeSecret, SecretString};

const CERT: [u8; 32] = [0xcd; 32];
const PEER: &str = "192.0.2.7:51000";

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

struct Counter(AtomicU8);
impl Entropy for Counter {
    fn fill(&self, buf: &mut [u8]) {
        for b in buf {
            *b = self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Users;
impl UserVerifier for Users {
    fn verify(&self, username: &str, password: &SecretString) -> bool {
        matches!(
            (username, password.expose_secret()),
            ("alice", "wonderland") | ("rj", "owner-pw")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Input {
    Key(bool, u32),
    Pointer(u8, u16, u16),
}

#[derive(Clone, Default)]
struct Recorder {
    input: Arc<Mutex<Vec<Input>>>,
    clipboard: Arc<Mutex<Vec<String>>>,
    resizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl InputSink for Recorder {
    fn key(&mut self, down: bool, keysym: u32) {
        self.input.lock().unwrap().push(Input::Key(down, keysym));
    }
    fn pointer(&mut self, buttons: u8, x: u16, y: u16) {
        self.input
            .lock()
            .unwrap()
            .push(Input::Pointer(buttons, x, y));
    }
}

impl ClipboardSink for Recorder {
    fn set_text(&mut self, text: &str) {
        self.clipboard.lock().unwrap().push(text.to_owned());
    }
}

/// Accepts every resize; the harness applies it to the desktop.
impl DesktopResizer for Recorder {
    fn resize(&mut self, width: u16, height: u16, _screens: &[Screen]) -> ResizeOutcome {
        self.resizes.lock().unwrap().push((width, height));
        ResizeOutcome::Accepted
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sec {
    None,
    VncAuth,
    X509Plain,
    X509Vnc,
    Ra2(Ra2Variant),
}

fn server_config(auth: Sec) -> ServerSessionConfig {
    let mut c = ServerSessionConfig::new(PEER, Arc::new(Counter(AtomicU8::new(0))));
    c.owner = Some("rj".into());
    match auth {
        Sec::None => {
            c.loopback = true;
            c.security_types = vec![SecurityType::None];
        }
        Sec::VncAuth => {
            c.loopback = true;
            c.security_types = vec![SecurityType::VncAuth];
            c.vnc_password = Some(secret("pw"));
        }
        Sec::X509Plain => {
            c.security_types = vec![SecurityType::VeNCrypt];
            c.tls_available = true;
            c.user_verifier = Some(Arc::new(Users));
        }
        Sec::X509Vnc => {
            c.security_types = vec![SecurityType::VeNCrypt];
            c.tls_available = true;
            c.vnc_password = Some(secret("pw"));
        }
        Sec::Ra2(variant) => {
            c.security_types = vec![match variant {
                Ra2Variant::Sha1 => SecurityType::Ra2,
                Ra2Variant::Sha256 => SecurityType::Ra256,
            }];
            c.ra2_key = Some(Arc::new(Ra2ServerKey::generate().unwrap()));
            c.user_verifier = Some(Arc::new(Users));
        }
    }
    c
}

fn viewer_config(auth: Sec) -> SessionConfig {
    let (username, password) = match auth {
        Sec::None => (None, None),
        Sec::VncAuth | Sec::X509Vnc => (None, Some(secret("pw"))),
        Sec::X509Plain | Sec::Ra2(_) => (Some(secret("alice")), Some(secret("wonderland"))),
    };
    SessionConfig {
        loopback: matches!(auth, Sec::None | Sec::VncAuth),
        username,
        password,
        shared: true,
        ..SessionConfig::default()
    }
}

fn gradient_desktop(width: u16, height: u16) -> Desktop {
    let mut desktop = Desktop::new(width, height, "test-desktop");
    let mut frame = Framebuffer::new(width, height);
    let stride = usize::from(width);
    frame
        .write(Rect::new(0, 0, width, height), |i| {
            let (x, y) = (i % stride, i / stride);
            let value = u32::try_from(x * 3 + y * 5).unwrap();
            if (x / 16 + y / 16) % 3 == 0 {
                0x0020_4060
            } else {
                value.wrapping_mul(2_654_435_761) & 0x00ff_ffff
            }
        })
        .unwrap();
    desktop.update(frame, None);
    desktop
}

struct World {
    viewer: Session,
    server: ServerSession,
    desktop: Desktop,
    recorder: Recorder,
    viewer_cipher: Option<Ra2Cipher>,
    server_cipher: Option<Ra2Cipher>,
    viewer_events: Vec<ViewerEvent>,
    server_events: Vec<SessionEvent>,
    /// Answer access prompts automatically with allow.
    auto_allow: bool,
    wire: Vec<u8>,
}

impl World {
    fn new(auth: Sec) -> Self {
        Self::with(
            server_config(auth),
            viewer_config(auth),
            gradient_desktop(100, 70),
        )
    }

    fn with(server: ServerSessionConfig, viewer: SessionConfig, desktop: Desktop) -> Self {
        let recorder = Recorder::default();
        let ports = SessionPorts {
            input: Box::new(recorder.clone()),
            clipboard: Box::new(recorder.clone()),
            resizer: Some(Box::new(recorder.clone())),
        };
        Self {
            viewer: Session::new(viewer),
            server: ServerSession::new(server, ports),
            desktop,
            recorder,
            viewer_cipher: None,
            server_cipher: None,
            viewer_events: Vec::new(),
            server_events: Vec::new(),
            auto_allow: true,
            wire: Vec::new(),
        }
    }

    fn run(&mut self) -> Result<(), ProtocolError> {
        for _ in 0..50 {
            self.server.frame_ready(&self.desktop)?;
            let out = self.server.take_output();
            let raw = seal(self.server_cipher.as_mut(), out);
            self.wire.extend(&raw);
            self.viewer
                .receive(&open(self.viewer_cipher.as_mut(), &raw));
            for event in self.viewer.poll()? {
                self.on_viewer_event(event)?;
            }
            let out = self.viewer.take_output();
            let raw = seal(self.viewer_cipher.as_mut(), out);
            self.wire.extend(&raw);
            self.server
                .receive(&open(self.server_cipher.as_mut(), &raw));
            for event in self.server.poll()? {
                self.on_server_event(event.clone())?;
                self.server_events.push(event);
            }
            self.apply_resizes();
        }
        Ok(())
    }

    fn apply_resizes(&mut self) {
        for (w, h) in self.recorder.resizes.lock().unwrap().drain(..) {
            self.desktop.resize(w, h);
        }
    }

    fn on_server_event(&mut self, event: SessionEvent) -> Result<(), ProtocolError> {
        match event {
            SessionEvent::UpgradeTransport(TransportUpgrade::Ra2 {
                send_key,
                receive_key,
            }) => {
                // Anything queued before the upgrade goes out in the clear.
                let pending = self.server.take_output();
                self.viewer.receive(&pending);
                self.server_cipher = Some(Ra2Cipher::new(&send_key, &receive_key).unwrap());
                self.server.transport_upgraded()
            }
            SessionEvent::UpgradeTransport(_) => self.server.transport_upgraded(),
            SessionEvent::ClientInit { .. } => self.server.admit(&self.desktop),
            SessionEvent::AccessRequest { .. } if self.auto_allow => {
                self.server.allow(&self.desktop)
            }
            _ => Ok(()),
        }
    }

    fn on_viewer_event(&mut self, event: ViewerEvent) -> Result<(), ProtocolError> {
        match &event {
            ViewerEvent::TransportUpgrade(TransportUpgrade::Tls) => {
                if self.viewer.tls_established(CERT, false)?.is_some() {
                    self.viewer.confirm_identity()?;
                }
            }
            ViewerEvent::TransportUpgrade(TransportUpgrade::Ra2 {
                send_key,
                receive_key,
            }) => {
                let pending = self.viewer.take_output();
                self.server.receive(&pending);
                self.viewer_cipher = Some(Ra2Cipher::new(send_key, receive_key).unwrap());
                self.viewer.transport_upgraded()?;
            }
            ViewerEvent::IdentityConfirmationRequired { .. } => self.viewer.confirm_identity()?,
            _ => {}
        }
        self.viewer_events.push(event);
        Ok(())
    }

    fn assert_in_sync(&self) {
        assert_eq!(self.viewer.framebuffer(), self.desktop.framebuffer());
    }

    fn damaged(&self) -> Vec<Rect> {
        self.viewer_events
            .iter()
            .filter_map(|e| match e {
                ViewerEvent::Damaged(r) => Some(*r),
                _ => None,
            })
            .collect()
    }

    fn inputs(&self) -> Vec<Input> {
        self.recorder.input.lock().unwrap().clone()
    }
}

fn seal(cipher: Option<&mut Ra2Cipher>, plain: Vec<u8>) -> Vec<u8> {
    match cipher {
        Some(c) if !plain.is_empty() => {
            let mut out = Vec::new();
            for chunk in plain.chunks(4096) {
                c.seal(chunk, &mut out).unwrap();
            }
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

const ALL_AUTH: [Sec; 6] = [
    Sec::None,
    Sec::VncAuth,
    Sec::X509Plain,
    Sec::X509Vnc,
    Sec::Ra2(Ra2Variant::Sha1),
    Sec::Ra2(Ra2Variant::Sha256),
];

#[test]
fn every_security_type_connects_and_shows_the_desktop() {
    for auth in ALL_AUTH {
        let mut w = World::new(auth);
        w.run().unwrap();
        assert_eq!(w.viewer.status(), SessionStatus::Connected, "{auth:?}");
        assert!(w.server.is_connected());
        w.assert_in_sync();
        let encrypted = !matches!(auth, Sec::None | Sec::VncAuth);
        assert_eq!(w.viewer.encrypted(), encrypted, "{auth:?}");
        if matches!(auth, Sec::Ra2(_)) {
            let wire = String::from_utf8_lossy(&w.wire).into_owned();
            assert!(!wire.contains("test-desktop"), "{auth:?} leaked the name");
            assert!(!wire.contains("wonderland"), "{auth:?} leaked the password");
        }
    }
}

#[test]
fn full_and_incremental_updates_match_for_every_encoding() {
    for encoding in [Encoding::Tight, Encoding::Zrle, Encoding::Raw] {
        let mut config = server_config(Sec::None);
        config.encodings = vec![encoding];
        let mut w = World::with(config, viewer_config(Sec::None), gradient_desktop(300, 280));
        w.run().unwrap();
        assert_eq!(w.server.encoding(), encoding);
        w.assert_in_sync();

        // A small change inside one tile: only that area is sent.
        w.viewer_events.clear();
        let patch: Vec<u32> = (0..20u32).map(|i| i * 0x0001_0203).collect();
        w.desktop
            .update_region(Rect::new(70, 70, 5, 4), &patch, None)
            .unwrap();
        w.run().unwrap();
        w.assert_in_sync();
        assert_eq!(w.damaged(), [Rect::new(70, 70, 5, 4)], "{encoding:?}");

        // Explicit damage from the adapter, spread over several tiles, and
        // a whole new frame.
        let mut frame = w.desktop.framebuffer().clone();
        frame.fill(Rect::new(0, 200, 300, 30), 0x00ff_00ff).unwrap();
        frame.fill(Rect::new(250, 0, 10, 10), 0x0000_ff00).unwrap();
        w.desktop.update(
            frame,
            Some(&[Rect::new(0, 200, 300, 30), Rect::new(250, 0, 10, 10)]),
        );
        w.run().unwrap();
        w.assert_in_sync();
        w.desktop
            .update(gradient_desktop(300, 280).framebuffer().clone(), None);
        w.desktop
            .update_region(Rect::new(0, 0, 1, 1), &[1], None)
            .unwrap();
        w.run().unwrap();
        w.assert_in_sync();
    }
}

#[test]
fn cursor_shape_reaches_the_viewer() {
    let mut w = World::new(Sec::None);
    w.run().unwrap();
    let rgba: Vec<u8> = (0..12u8 * 10)
        .flat_map(|i| {
            if i % 3 == 0 {
                [0, 0, 0, 0]
            } else {
                [i, 255 - i, i / 2, 255]
            }
        })
        .collect();
    let cursor = Cursor::from_rgba(12, 10, (3, 4), rgba).unwrap();
    w.desktop.set_cursor(Some(cursor.clone()));
    w.run().unwrap();
    assert_eq!(w.viewer.decoder().cursor(), Some(&cursor));
    assert!(w.viewer_events.contains(&ViewerEvent::CursorChanged));

    // Hiding the pointer sends an empty shape.
    w.desktop.set_cursor(None);
    w.run().unwrap();
    assert!(w.viewer.decoder().cursor().unwrap().is_empty());
}

#[test]
fn desktop_resize_reaches_the_viewer() {
    let mut w = World::new(Sec::None);
    w.run().unwrap();
    // The first update told the viewer that SetDesktopSize is understood.
    assert!(w.viewer.decoder().supports_extended_resize());
    w.desktop
        .update(gradient_desktop(160, 90).framebuffer().clone(), None);
    w.run().unwrap();
    assert!(w.viewer_events.contains(&ViewerEvent::Resized {
        width: 160,
        height: 90
    }));
    w.assert_in_sync();
    assert_eq!(w.viewer.decoder().screens()[0].width, 160);
}

#[test]
fn viewer_resize_requests_are_prohibited_on_live_desktops() {
    let mut config = server_config(Sec::None);
    config.encodings = vec![Encoding::Zrle];
    let recorder = Recorder::default();
    let mut w = World::with(config, viewer_config(Sec::None), gradient_desktop(100, 70));
    w.server = ServerSession::new(
        {
            let mut c = server_config(Sec::None);
            c.encodings = vec![Encoding::Zrle];
            c
        },
        SessionPorts {
            input: Box::new(recorder.clone()),
            clipboard: Box::new(recorder),
            resizer: None,
        },
    );
    w.run().unwrap();
    assert!(w.viewer.request_resize(200, 100));
    w.run().unwrap();
    let last = w.viewer.decoder().last_resize().unwrap();
    assert_eq!(last.reason, ResizeReason::ThisClient);
    assert_eq!(last.status, ResizeStatus::Prohibited);
    assert_eq!(w.viewer.framebuffer().width(), 100);
    w.assert_in_sync();
}

#[test]
fn a_resize_hook_can_accept_viewer_resizes() {
    let mut w = World::new(Sec::None);
    w.run().unwrap();
    assert!(w.viewer.request_resize(200, 120));
    w.run().unwrap();
    let last = w.viewer.decoder().last_resize().unwrap();
    assert_eq!(last.status, ResizeStatus::Ok);
    assert_eq!(w.desktop.framebuffer().width(), 200);
    w.assert_in_sync();
}

#[test]
fn input_reaches_the_input_sink() {
    let mut w = World::new(Sec::X509Plain);
    w.run().unwrap();
    w.viewer.key(true, 0x61);
    w.viewer.key(false, 0x61);
    w.viewer.pointer(1, 10, 20);
    w.run().unwrap();
    assert_eq!(
        w.inputs(),
        [
            Input::Key(true, 0x61),
            Input::Key(false, 0x61),
            Input::Pointer(1, 10, 20)
        ]
    );
}

#[test]
fn view_only_drops_input_and_clipboard_on_the_server() {
    let mut config = server_config(Sec::X509Plain);
    config.view_only = true;
    let mut w = World::with(
        config,
        viewer_config(Sec::X509Plain),
        gradient_desktop(64, 64),
    );
    w.run().unwrap();
    assert!(w.server_events.iter().any(|e| matches!(
        e,
        SessionEvent::Connected {
            view_only: true,
            ..
        }
    )));
    // The viewer itself is not view-only: the agent must enforce it.
    w.viewer.key(true, 0x61);
    w.viewer.pointer(1, 1, 1);
    w.viewer.clipboard("sneaky");
    w.run().unwrap();
    assert_eq!(w.inputs(), []);
    assert!(w.recorder.clipboard.lock().unwrap().is_empty());
    // Switching view-only off at runtime lets input through.
    w.server.set_view_only(false);
    w.viewer.key(true, 0x62);
    w.run().unwrap();
    assert_eq!(w.inputs(), [Input::Key(true, 0x62)]);
}

#[test]
fn clipboard_flows_both_ways_without_echo() {
    let mut w = World::new(Sec::Ra2(Ra2Variant::Sha256));
    w.run().unwrap();
    w.desktop.set_clipboard("café ☕");
    w.run().unwrap();
    assert!(
        w.viewer_events
            .contains(&ViewerEvent::Clipboard("café ?".into()))
    );

    w.viewer_events.clear();
    w.viewer.clipboard("from the viewer");
    w.run().unwrap();
    assert_eq!(
        *w.recorder.clipboard.lock().unwrap(),
        ["from the viewer".to_owned()]
    );
    // The adapter puts it on the desktop clipboard; it is not echoed back.
    w.desktop.set_clipboard("from the viewer");
    w.run().unwrap();
    assert!(
        !w.viewer_events
            .iter()
            .any(|e| matches!(e, ViewerEvent::Clipboard(_)))
    );
}

#[test]
fn oversized_clipboard_is_not_sent() {
    let mut config = server_config(Sec::None);
    config.max_clipboard_out = 8;
    let mut w = World::with(config, viewer_config(Sec::None), gradient_desktop(32, 32));
    w.run().unwrap();
    w.desktop.set_clipboard("much longer than eight bytes");
    w.run().unwrap();
    assert!(
        !w.viewer_events
            .iter()
            .any(|e| matches!(e, ViewerEvent::Clipboard(_)))
    );
}

fn access_requests(w: &World) -> Vec<(Option<String>, String)> {
    w.server_events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::AccessRequest { username, peer } => {
                Some((username.clone(), peer.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_owner_skips_the_access_prompt() {
    let mut viewer = viewer_config(Sec::X509Plain);
    viewer.username = Some(secret("rj"));
    viewer.password = Some(secret("owner-pw"));
    let mut w = World::with(
        server_config(Sec::X509Plain),
        viewer,
        gradient_desktop(32, 32),
    );
    w.auto_allow = false;
    w.run().unwrap();
    assert_eq!(access_requests(&w), []);
    assert_eq!(w.viewer.status(), SessionStatus::Connected);
    assert!(w.server.is_owner());
    assert!(w.server_events.contains(&SessionEvent::Connected {
        security: SecurityType::VeNCrypt,
        username: Some("rj".into()),
        owner: true,
        view_only: false,
    }));
}

#[test]
fn other_users_are_prompted_and_wait_for_approval() {
    for auth in [Sec::X509Plain, Sec::Ra2(Ra2Variant::Sha1), Sec::VncAuth] {
        let mut w = World::new(auth);
        w.auto_allow = false;
        w.run().unwrap();
        let username = (auth != Sec::VncAuth).then(|| "alice".to_owned());
        assert_eq!(
            access_requests(&w),
            [(username, PEER.to_owned())],
            "{auth:?}"
        );
        // No ServerInit (and so no desktop name or pixels) before approval.
        assert_ne!(w.viewer.status(), SessionStatus::Connected);
        assert_eq!(w.viewer.framebuffer().width(), 0);
        w.server.allow(&w.desktop).unwrap();
        w.run().unwrap();
        assert_eq!(w.viewer.status(), SessionStatus::Connected);
        w.assert_in_sync();
    }
}

#[test]
fn denying_the_prompt_closes_the_connection() {
    let mut w = World::new(Sec::X509Plain);
    w.auto_allow = false;
    w.run().unwrap();
    w.server.deny();
    assert!(w.server.is_closed());
    w.run().unwrap();
    assert_ne!(w.viewer.status(), SessionStatus::Connected);
    assert!(w.server.allow(&w.desktop).is_err());
}

#[test]
fn security_offer_follows_policy_and_material() {
    let entropy: Arc<dyn Entropy> = Arc::new(Counter(AtomicU8::new(0)));
    let mut c = ServerSessionConfig::new(PEER, Arc::clone(&entropy));
    c.security_types = vec![
        SecurityType::VeNCrypt,
        SecurityType::Ra256,
        SecurityType::Ra2,
        SecurityType::VncAuth,
        SecurityType::None,
    ];
    // Nothing configured: only None, and that only on loopback.
    assert_eq!(c.offer(), []);
    c.loopback = true;
    assert_eq!(c.offer(), [SecurityType::None]);
    c.loopback = false;
    c.vnc_password = Some(secret("pw"));
    c.tls_available = true;
    c.ra2_key = Some(Arc::new(Ra2ServerKey::generate().unwrap()));
    assert_eq!(
        c.offer(),
        [
            SecurityType::VeNCrypt,
            SecurityType::Ra256,
            SecurityType::Ra2
        ]
    );
    c.allow_legacy = true;
    assert_eq!(c.offer().len(), 5);
    let rendered = format!("{c:?}");
    assert!(!rendered.contains("pw\""), "{rendered}");
}

fn ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7))
}

#[test]
fn repeated_wrong_passwords_lock_the_address_out() {
    let mut access = AccessControl::new(AccessPolicy::DEFAULT);
    let mut now = 0;
    for attempt in 0..4 {
        let id = access.connect(ip(), now).unwrap();
        let mut viewer = viewer_config(Sec::VncAuth);
        viewer.password = Some(secret("wrong"));
        let mut w = World::with(server_config(Sec::VncAuth), viewer, gradient_desktop(8, 8));
        let error = w.run().unwrap_err();
        // The server reports the failure; the viewer sees the reason.
        assert!(
            matches!(error, ProtocolError::Auth(_)),
            "{attempt}: {error}"
        );
        access.auth_failed(ip(), now);
        access.disconnected(id);
        now += 10;
    }
    assert_eq!(
        access.connect(ip(), now),
        Err(Refusal::LockedOut {
            retry_after_ms: 1_990
        })
    );
    assert!(access.connect(ip(), now + 2_000).is_ok());
}

#[test]
fn connection_limit_and_exclusive_access() {
    let mut access = AccessControl::new(AccessPolicy {
        max_connections: 2,
        ..AccessPolicy::DEFAULT
    });
    let first_id = access.connect(ip(), 0).unwrap();
    let mut first = World::new(Sec::None);
    first.run().unwrap();
    assert_eq!(
        access.admit(first_id, first.server.shared()),
        Admission::Admit { disconnect: vec![] }
    );

    // A second viewer asking for exclusive access is refused (default policy).
    let second_id = access.connect(ip(), 0).unwrap();
    let mut viewer = viewer_config(Sec::None);
    viewer.shared = false;
    let mut second = World::with(server_config(Sec::None), viewer, gradient_desktop(8, 8));
    // Drive by hand: stop at ClientInit and ask AccessControl.
    second.server.receive(&[]);
    for _ in 0..10 {
        let out = second.server.take_output();
        second.viewer.receive(&out);
        second.viewer.poll().unwrap();
        let out = second.viewer.take_output();
        second.server.receive(&out);
        for event in second.server.poll().unwrap() {
            if let SessionEvent::ClientInit { shared } = event {
                assert!(!shared);
                assert_eq!(access.admit(second_id, shared), Admission::Refuse);
                second.server.deny();
            }
        }
    }
    assert!(second.server.is_closed());
    assert_ne!(second.viewer.status(), SessionStatus::Connected);

    // The limit counts every open connection.
    assert_eq!(access.connect(ip(), 0), Err(Refusal::TooManyConnections));
    access.disconnected(second_id);
    assert!(access.connect(ip(), 0).is_ok());
}

/// A viewer using a 16-bit and then an 8-bit colour-map pixel format,
/// driven through the raw protocol machine.
#[test]
#[allow(clippy::too_many_lines)] // One linear protocol script.
fn rgb565_and_colour_map_viewers_get_converted_pixels() {
    const RGB565: PixelFormat = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: true,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };
    const MAPPED: PixelFormat = PixelFormat {
        bits_per_pixel: 8,
        depth: 8,
        big_endian: false,
        true_colour: false,
        red_max: 0,
        green_max: 0,
        blue_max: 0,
        red_shift: 0,
        green_shift: 0,
        blue_shift: 0,
    };
    for (format, encoding) in [
        (RGB565, Encoding::Zrle),
        (RGB565, Encoding::Tight),
        (MAPPED, Encoding::Zrle),
        (MAPPED, Encoding::Tight),
        (MAPPED, Encoding::Raw),
    ] {
        let desktop = gradient_desktop(90, 70);
        let recorder = Recorder::default();
        let mut server = ServerSession::new(
            server_config(Sec::None),
            SessionPorts {
                input: Box::new(recorder.clone()),
                clipboard: Box::new(recorder),
                resizer: None,
            },
        );
        let mut client = ClientConnection::new(ClientConfig::default());
        let mut decoder = Decoder::new(0, 0, PixelFormat::RGB888, Limits::DEFAULT);
        let mut colour_map = Vec::new();
        for _ in 0..20 {
            server.frame_ready(&desktop).unwrap();
            client.receive(&server.take_output());
            while let Some(event) = client.poll(&mut decoder).unwrap() {
                match event {
                    ClientEvent::SecurityTypes(_) => client
                        .choose_security(SecurityType::None, Box::new(NoAuth))
                        .unwrap(),
                    ClientEvent::Connected(init) => {
                        decoder = Decoder::new(init.width, init.height, format, Limits::DEFAULT);
                        for message in [
                            ClientMessage::SetPixelFormat(format),
                            ClientMessage::SetEncodings(vec![encoding]),
                            ClientMessage::FramebufferUpdateRequest {
                                incremental: false,
                                rect: Rect::new(0, 0, init.width, init.height),
                            },
                        ] {
                            client.send(&message).unwrap();
                        }
                    }
                    ClientEvent::ColourMap { first, colours } => {
                        decoder.set_colour_map(first, &colours);
                        colour_map = colours;
                    }
                    _ => {}
                }
            }
            server.receive(&client.take_output());
            for event in server.poll().unwrap() {
                match event {
                    SessionEvent::ClientInit { .. } => server.admit(&desktop).unwrap(),
                    SessionEvent::AccessRequest { .. } => server.allow(&desktop).unwrap(),
                    _ => {}
                }
            }
        }
        // What each desktop pixel should look like on this viewer.
        let expect = |rgb: u32| -> u32 {
            if format.true_colour {
                let conv = PixelConverter::new(format);
                let mut wire = Vec::new();
                conv.encode(rgb, &mut wire);
                conv.decode(&wire)
            } else {
                let r = (rgb >> 16) & 0xff;
                let g = (rgb >> 8) & 0xff;
                let b = rgb & 0xff;
                let index = ((r * 7 + 127) / 255)
                    | (((g * 7 + 127) / 255) << 3)
                    | (((b * 3 + 127) / 255) << 6);
                let [r, g, b] = colour_map[usize::try_from(index).unwrap()];
                (u32::from(r >> 8) << 16) | (u32::from(g >> 8) << 8) | u32::from(b >> 8)
            }
        };
        assert_eq!(format.true_colour, colour_map.is_empty());
        let want: Vec<u32> = desktop
            .framebuffer()
            .pixels()
            .iter()
            .map(|&p| expect(p))
            .collect();
        assert_eq!(
            decoder.framebuffer().pixels(),
            want,
            "{format:?} {encoding:?}"
        );
    }
}

#[test]
fn a_slow_viewer_catches_up_without_holding_back_a_fast_one() {
    let mut fast = World::new(Sec::None);
    let mut slow = World::new(Sec::None);
    fast.run().unwrap();
    slow.run().unwrap();
    for i in 0..100u32 {
        let x = u16::try_from(i % 90).unwrap();
        fast.desktop
            .update_region(Rect::new(x, 5, 3, 3), &[i * 77; 9], None)
            .unwrap();
        fast.run().unwrap();
        fast.assert_in_sync();
        // The slow viewer's session sees the same desktop but never runs.
        slow.desktop = fast.desktop.clone();
    }
    slow.run().unwrap();
    slow.assert_in_sync();
}

#[test]
fn nothing_is_sent_while_the_desktop_is_unchanged() {
    let mut w = World::new(Sec::Ra2(Ra2Variant::Sha1));
    w.run().unwrap();
    // The viewer has an incremental request outstanding; nothing changed.
    w.server.frame_ready(&w.desktop).unwrap();
    assert_eq!(w.server.take_output(), []);
    w.desktop.update(w.desktop.framebuffer().clone(), None);
    w.server.frame_ready(&w.desktop).unwrap();
    assert_eq!(w.server.take_output(), []);
}
