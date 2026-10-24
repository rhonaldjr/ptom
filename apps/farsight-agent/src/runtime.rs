//! The agent at run time (ADR-0018): one shared desktop fed by the capture
//! backend, one task per viewer driving a `ServerSession` over TCP (with TLS
//! or RA2 upgrades), access control, the approval prompt (U-2) and the
//! presence indicator (U-4).

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use anyhow::{Context, Result};
use farsight_desktop::notify;
use farsight_desktop::presence::Presence;
use farsight_net::Transport;
use farsight_rfb::ProtocolError;
use farsight_rfb::auth::TransportUpgrade;
use farsight_security::ra2::Ra2ServerKey;
use farsight_server::{
    AccessControl, AccessPolicy, Admission, ClipboardSink, ConnectionId, Desktop, Entropy,
    InputSink, ServerSession, ServerSessionConfig, SessionEvent, SessionPorts, UserVerifier,
};
use secrecy::SecretString;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

use crate::capture::{Capture, Event, Input};

/// Fixed settings for every viewer.
pub struct Settings {
    pub owner: Option<String>,
    pub tls: TlsAcceptor,
    pub ra2: Arc<Ra2ServerKey>,
    pub verifier: Option<Arc<dyn UserVerifier>>,
    pub vnc_password: Option<SecretString>,
    pub allow_legacy: bool,
    pub view_only: bool,
}

/// State shared by all viewer tasks.
pub struct Agent {
    settings: Settings,
    desktop: Mutex<Desktop>,
    version: watch::Sender<u64>,
    clipboard: broadcast::Sender<String>,
    access: Mutex<AccessControl>,
    kills: Mutex<BTreeMap<ConnectionId, Arc<Notify>>>,
    viewers: Mutex<BTreeMap<ConnectionId, String>>,
    presence: tokio::sync::OnceCell<Presence>,
    input: Input,
    started: Instant,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Agent {
    pub fn new(settings: Settings, desktop_name: String, input: Input) -> Arc<Self> {
        Arc::new(Self {
            settings,
            desktop: Mutex::new(Desktop::new(0, 0, desktop_name)),
            version: watch::channel(0).0,
            clipboard: broadcast::channel(4).0,
            access: Mutex::new(AccessControl::new(AccessPolicy::DEFAULT)),
            kills: Mutex::new(BTreeMap::new()),
            viewers: Mutex::new(BTreeMap::new()),
            presence: tokio::sync::OnceCell::new(),
            input,
            started: Instant::now(),
        })
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Starts the tray icon; "Disconnect everyone" ends every session.
    pub async fn start_presence(self: &Arc<Self>) {
        let agent = Arc::downgrade(self);
        let presence = Presence::start(Arc::new(move || {
            if let Some(agent) = agent.upgrade() {
                agent.disconnect_all();
            }
        }))
        .await;
        let _ = self.presence.set(presence);
    }

    fn disconnect_all(&self) {
        for kill in lock(&self.kills).values() {
            kill.notify_one();
        }
    }

    async fn update_presence(&self) {
        let viewers: Vec<String> = lock(&self.viewers).values().cloned().collect();
        if let Some(presence) = self.presence.get() {
            presence.set_viewers(viewers).await;
        }
    }

    /// Feeds the shared desktop from the backend until it closes.
    pub async fn pump(self: Arc<Self>, mut capture: Capture) -> Result<()> {
        while let Some(event) = capture.events.recv().await {
            match event {
                Event::Frame(framebuffer, damage) => {
                    lock(&self.desktop).update(framebuffer, damage.as_deref());
                    self.version.send_modify(|v| *v = v.wrapping_add(1));
                }
                Event::Cursor(cursor) => {
                    lock(&self.desktop).set_cursor(cursor);
                    self.version.send_modify(|v| *v = v.wrapping_add(1));
                }
                Event::Clipboard(text) => {
                    let _ = self.clipboard.send(text);
                }
                Event::Closed(reason) => anyhow::bail!("the desktop went away: {reason}"),
            }
        }
        Ok(())
    }

    /// Accepts viewers forever.
    pub async fn listen(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        loop {
            let (stream, peer) = listener.accept().await.context("accepting a connection")?;
            let _ = stream.set_nodelay(true);
            tokio::spawn(Arc::clone(&self).serve(stream, peer));
        }
    }

    async fn serve(self: Arc<Self>, stream: TcpStream, peer: SocketAddr) {
        let ip = peer.ip();
        let id = match lock(&self.access).connect(ip, self.now_ms()) {
            Ok(id) => id,
            Err(refusal) => {
                info!(%peer, ?refusal, "connection refused");
                return;
            }
        };
        let kill = Arc::new(Notify::new());
        lock(&self.kills).insert(id, Arc::clone(&kill));
        if let Err(error) = self.run_session(id, stream, peer, &kill).await {
            info!(%peer, %error, "viewer disconnected");
        }
        lock(&self.access).disconnected(id);
        lock(&self.kills).remove(&id);
        let left = lock(&self.viewers).remove(&id);
        if let Some(viewer) = left {
            self.update_presence().await;
            notify::announce("A viewer disconnected", &viewer).await;
        }
    }

    fn session_config(&self, peer: SocketAddr) -> ServerSessionConfig {
        let mut config = ServerSessionConfig::new(peer.to_string(), Arc::new(OsEntropy));
        config.loopback = peer.ip().is_loopback() || is_mapped_loopback(peer.ip());
        config.allow_legacy = self.settings.allow_legacy;
        config.tls_available = true;
        config.ra2_key = Some(Arc::clone(&self.settings.ra2));
        config.user_verifier.clone_from(&self.settings.verifier);
        config.vnc_password.clone_from(&self.settings.vnc_password);
        config.owner.clone_from(&self.settings.owner);
        config.view_only = self.settings.view_only;
        config
    }

    async fn run_session(
        &self,
        id: ConnectionId,
        stream: TcpStream,
        peer: SocketAddr,
        kill: &Notify,
    ) -> Result<()> {
        let ports = SessionPorts {
            input: Box::new(InputBridge(self.input.clone())),
            clipboard: Box::new(ClipboardBridge(self.input.clone())),
            resizer: None,
        };
        let mut session = ServerSession::new(self.session_config(peer), ports);
        let mut transport = Transport::new(stream);
        let mut desktop_changes = self.version.subscribe();
        let mut clipboard = self.clipboard.subscribe();
        let (approval_tx, mut approval) = mpsc::channel::<bool>(1);
        let mut buf = vec![0u8; 64 * 1024];
        transport.write(&session.take_output()).await?;
        loop {
            let events = tokio::select! {
                read = transport.read(&mut buf) => {
                    let Some(bytes) = read? else { return Ok(()) };
                    session.receive(&bytes);
                    self.poll(&mut session, &mut transport, peer.ip()).await?
                }
                changed = desktop_changes.changed(), if session.is_connected() => {
                    changed?;
                    session.frame_ready(&lock(&self.desktop))?;
                    Vec::new()
                }
                text = clipboard.recv(), if session.is_connected() => {
                    if let Ok(text) = text {
                        session.send_clipboard(&text)?;
                    }
                    Vec::new()
                }
                Some(allowed) = approval.recv() => {
                    if allowed {
                        session.allow(&lock(&self.desktop))?;
                        session.poll()?
                    } else {
                        session.deny();
                        transport.write(&session.take_output()).await?;
                        return Ok(());
                    }
                }
                () = kill.notified() => return Ok(()),
            };
            for event in events {
                self.on_event(id, &mut session, event, &approval_tx).await?;
            }
            // A viewer's update request may be answerable right away.
            if session.is_connected() {
                session.frame_ready(&lock(&self.desktop))?;
            }
            transport.write(&session.take_output()).await?;
            if session.is_closed() {
                return Ok(());
            }
        }
    }

    /// Polls the session, performing transport upgrades in place.
    async fn poll(
        &self,
        session: &mut ServerSession,
        transport: &mut Transport,
        ip: IpAddr,
    ) -> Result<Vec<SessionEvent>> {
        let mut out = Vec::new();
        loop {
            let events = match session.poll() {
                Ok(events) => events,
                Err(ProtocolError::Auth(error)) => {
                    lock(&self.access).auth_failed(ip, self.now_ms());
                    transport.write(&session.take_output()).await?;
                    return Err(error.into());
                }
                Err(error) => {
                    transport.write(&session.take_output()).await?;
                    return Err(error.into());
                }
            };
            let mut upgraded = false;
            for event in events {
                match event {
                    SessionEvent::UpgradeTransport(upgrade) => {
                        // Bytes queued before the upgrade go out in the clear.
                        transport.write(&session.take_output()).await?;
                        match upgrade {
                            TransportUpgrade::Tls => {
                                transport.accept_tls(&self.settings.tls).await?;
                            }
                            TransportUpgrade::Ra2 {
                                send_key,
                                receive_key,
                            } => {
                                transport.enable_ra2(&send_key, &receive_key)?;
                            }
                            _ => anyhow::bail!("unsupported transport upgrade"),
                        }
                        session.transport_upgraded()?;
                        upgraded = true;
                    }
                    SessionEvent::Authenticated { .. } => lock(&self.access).auth_succeeded(ip),
                    other => out.push(other),
                }
            }
            if !upgraded {
                return Ok(out);
            }
        }
    }

    async fn on_event(
        &self,
        id: ConnectionId,
        session: &mut ServerSession,
        event: SessionEvent,
        approval: &mpsc::Sender<bool>,
    ) -> Result<()> {
        match event {
            SessionEvent::ClientInit { shared } => {
                let admission = lock(&self.access).admit(id, shared);
                match admission {
                    Admission::Admit { disconnect } => {
                        let kills = lock(&self.kills);
                        for other in disconnect {
                            if let Some(kill) = kills.get(&other) {
                                kill.notify_one();
                            }
                        }
                        drop(kills);
                        session.admit(&lock(&self.desktop))?;
                    }
                    Admission::Refuse => session.deny(),
                }
            }
            SessionEvent::AccessRequest { username, peer } => {
                let who = username.map_or_else(|| peer.clone(), |u| format!("{u} ({peer})"));
                let approval = approval.clone();
                tokio::spawn(async move {
                    let allowed = notify::ask_access(&who).await;
                    let _ = approval.send(allowed).await;
                });
            }
            SessionEvent::Connected {
                username,
                owner,
                view_only,
                ..
            } => {
                let mut label = username.unwrap_or_else(|| session.peer().to_owned());
                if owner {
                    label.push_str(" (you)");
                }
                if view_only {
                    label.push_str(", view only");
                }
                lock(&self.viewers).insert(id, label.clone());
                self.update_presence().await;
                notify::announce("Someone is viewing your screen", &label).await;
                info!(viewer = %label, "viewer connected");
            }
            SessionEvent::UpgradeTransport(_) | SessionEvent::Authenticated { .. } => {}
        }
        Ok(())
    }
}

fn is_mapped_loopback(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()))
}

/// Randomness from the operating system (via aws-lc-rs).
struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&self, buf: &mut [u8]) {
        use aws_lc_rs::rand::SecureRandom;
        if aws_lc_rs::rand::SystemRandom::new().fill(buf).is_err() {
            // Without randomness no challenge is safe: fail closed.
            warn!("the system random number generator failed; stopping");
            std::process::abort();
        }
    }
}

struct InputBridge(Input);

impl InputSink for InputBridge {
    fn key(&mut self, down: bool, keysym: u32) {
        self.0.key(down, keysym);
    }

    fn pointer(&mut self, buttons: u8, x: u16, y: u16) {
        self.0.pointer(buttons, x, y);
    }
}

struct ClipboardBridge(Input);

impl ClipboardSink for ClipboardBridge {
    fn set_text(&mut self, text: &str) {
        self.0.set_clipboard(text);
    }
}

/// Checks agent user passwords (ADR-0014).
pub struct CredentialsVerifier(pub farsight_vault::agent::AgentCredentials);

impl UserVerifier for CredentialsVerifier {
    fn verify(&self, username: &str, password: &SecretString) -> bool {
        use secrecy::ExposeSecret;
        self.0.verify(username, password.expose_secret().as_bytes())
    }
}
