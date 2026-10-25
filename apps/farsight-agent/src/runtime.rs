//! The agent at run time (ADR-0018): one shared desktop fed by the capture
//! backend, one task per viewer driving a `ServerSession` over TCP (with TLS
//! or RA2 upgrades), access control, the approval prompt (U-2) and the
//! presence indicator (U-4).
//!
//! Hardening from the persona review (2026-10-24): unfinished logins have a
//! deadline, writes time out, "Disconnect everyone" aborts tasks and pauses
//! sharing, only one approval prompt can be open, and a denied or unanswered
//! prompt counts as a failed login.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use farsight_desktop::notify;
use farsight_desktop::presence::Presence;
use farsight_net::Transport;
use farsight_rfb::Screen;
use farsight_rfb::auth::TransportUpgrade;
use farsight_rfb::{ProtocolError, SecurityType};
use farsight_security::ra2::Ra2ServerKey;
use farsight_server::{
    AccessControl, AccessPolicy, Admission, ClipboardSink, ConnectionId, Control, Decision,
    Desktop, DesktopResizer, Entropy, InputSink, ResizeOutcome, Role, ServerSession,
    ServerSessionConfig, SessionEvent, SessionPorts, UserVerifier,
};
use secrecy::SecretString;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};

use crate::capture::{Capture, Event, Input};

/// Time a viewer has from connecting to being let in (or asked about),
/// TLS handshake included.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(15);
/// Time any single write may take before the viewer is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Fewest milliseconds between two resizes of a headless desktop: dragging a
/// viewer window sends many; each one reallocates the whole desktop.
const MIN_RESIZE_INTERVAL_MS: u64 = 250;
/// How long sharing stays paused after "Disconnect everyone".
const PAUSE_AFTER_DISCONNECT_ALL: Duration = Duration::from_secs(120);

/// Fixed settings for every viewer.
pub struct Settings {
    pub owner: Option<String>,
    pub tls: TlsAcceptor,
    pub ra2: Arc<Ra2ServerKey>,
    pub logins: Logins,
    pub allow_legacy: bool,
    pub view_only: bool,
    /// A headless desktop (M7): nobody sits at it, so there's no approval
    /// prompt, viewers can resize it, and one viewer at a time controls it (U-9).
    pub headless: bool,
}

/// How viewers may log in; reloadable at run time (SIGHUP, M9).
#[derive(Clone, Default)]
pub struct Logins {
    pub verifier: Option<Arc<dyn UserVerifier>>,
    pub vnc_password: Option<SecretString>,
}

impl Logins {
    /// Whether anyone can log in with a password.
    pub fn any(&self) -> bool {
        self.verifier.is_some() || self.vnc_password.is_some()
    }
}

/// State shared by all viewer tasks.
pub struct Agent {
    settings: Settings,
    /// Current logins (starts as `settings.logins`, replaced by reloads).
    logins: Mutex<Logins>,
    desktop: Mutex<Desktop>,
    version: watch::Sender<u64>,
    clipboard: broadcast::Sender<String>,
    access: Mutex<AccessControl>,
    tasks: Mutex<BTreeMap<ConnectionId, AbortHandle>>,
    viewers: Mutex<BTreeMap<ConnectionId, String>>,
    prompt_open: Arc<AtomicBool>,
    presence: tokio::sync::OnceCell<Presence>,
    input: Input,
    /// Who controls a shared headless desktop (U-9); `None` on live desktops.
    control: Option<Mutex<Control>>,
    /// Frames rendered before the latest resize are dropped (headless).
    frame_generation: std::sync::atomic::AtomicU64,
    /// When the desktop was last resized (ms since start), for rate limiting.
    last_resize_ms: Mutex<Option<u64>>,
    started: Instant,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Writes with a timeout, so a viewer that stops reading can't hold its slot.
async fn write(transport: &mut Transport, bytes: &[u8]) -> Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, transport.write(bytes))
        .await
        .context("the viewer stopped receiving")??;
    Ok(())
}

impl Agent {
    pub fn new(settings: Settings, desktop_name: String, input: Input) -> Arc<Self> {
        let settings_headless = settings.headless;
        let logins = Mutex::new(settings.logins.clone());
        Arc::new(Self {
            logins,
            settings,
            desktop: Mutex::new(Desktop::new(0, 0, desktop_name)),
            version: watch::channel(0).0,
            clipboard: broadcast::channel(4).0,
            access: Mutex::new(AccessControl::new(AccessPolicy::DEFAULT)),
            tasks: Mutex::new(BTreeMap::new()),
            viewers: Mutex::new(BTreeMap::new()),
            prompt_open: Arc::new(AtomicBool::new(false)),
            presence: tokio::sync::OnceCell::new(),
            input,
            control: settings_headless.then(|| Mutex::new(Control::default())),
            frame_generation: std::sync::atomic::AtomicU64::new(0),
            last_resize_ms: Mutex::new(None),
            started: Instant::now(),
        })
    }

    fn now_ms(&self) -> u64 {
        millis(self.started.elapsed())
    }

    /// Replaces how viewers log in (passwords changed with
    /// `farsight-passwd`); connected viewers stay. A headless desktop keeps
    /// its old logins rather than lose its password.
    pub fn reload_logins(&self, logins: Logins) {
        if self.settings.headless && !logins.any() {
            warn!("not reloading: a headless desktop needs a password");
            return;
        }
        *lock(&self.logins) = logins;
        info!("reloaded the agent's passwords");
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

    /// Ends every session at once (even ones stuck writing) and refuses new
    /// connections for a while, so nobody can simply reconnect.
    fn disconnect_all(self: &Arc<Self>) {
        let until = self
            .now_ms()
            .saturating_add(millis(PAUSE_AFTER_DISCONNECT_ALL));
        lock(&self.access).pause_until(until);
        for task in lock(&self.tasks).values() {
            task.abort();
        }
        let agent = Arc::clone(self);
        tokio::spawn(async move {
            lock(&agent.viewers).clear();
            agent.update_presence().await;
            let minutes = PAUSE_AFTER_DISCONNECT_ALL.as_secs() / 60;
            notify::announce(
                "Everyone was disconnected",
                &format!("Sharing is paused for {minutes} minutes."),
            )
            .await;
        });
    }

    async fn update_presence(&self) {
        let viewers: Vec<String> = lock(&self.viewers).values().cloned().collect();
        if let Input::Headless(desktop) = &self.input {
            desktop.set_viewers(viewers.len());
        }
        if let Some(presence) = self.presence.get() {
            presence.set_viewers(viewers).await;
        }
    }

    /// Feeds the shared desktop from the backend until it closes.
    pub async fn pump(self: Arc<Self>, mut capture: Capture) -> Result<()> {
        while let Some(event) = capture.events.recv().await {
            match event {
                // A frame from before the latest resize would undo it.
                Event::Frame(_, _, generation)
                    if generation < self.frame_generation.load(Ordering::Acquire) => {}
                Event::Frame(framebuffer, damage, _) => {
                    lock(&self.desktop).update(framebuffer, damage.as_deref());
                    self.version.send_modify(|v| *v = v.wrapping_add(1));
                }
                Event::Cursor(cursor) => {
                    lock(&self.desktop).set_cursor(cursor);
                    self.version.send_modify(|v| *v = v.wrapping_add(1));
                }
                Event::Leds(leds) => {
                    lock(&self.desktop).set_leds(leds);
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

    /// Accepts viewers until the agent stops. Accept errors (e.g. running
    /// out of file descriptors) are logged and retried, never fatal.
    pub async fn listen(self: Arc<Self>, listener: TcpListener) -> Result<()> {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    warn!(%error, "could not accept a connection");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let connected = lock(&self.access).connect(peer.ip(), self.now_ms());
            let id = match connected {
                Ok(id) => id,
                Err(refusal) => {
                    info!(%peer, ?refusal, "connection refused");
                    continue;
                }
            };
            let task = tokio::spawn(Arc::clone(&self).run_session(id, stream, peer));
            lock(&self.tasks).insert(id, task.abort_handle());
            tokio::spawn(Arc::clone(&self).finish(id, peer, task));
        }
    }

    /// Waits for a viewer's task (finished or aborted) and cleans up.
    async fn finish(
        self: Arc<Self>,
        id: ConnectionId,
        peer: SocketAddr,
        task: JoinHandle<Result<()>>,
    ) {
        match task.await {
            Ok(Err(error)) => info!(%peer, %error, "viewer disconnected"),
            Err(join) if join.is_cancelled() => info!(%peer, "viewer disconnected by the agent"),
            _ => {}
        }
        lock(&self.access).disconnected(id);
        if let Some(control) = &self.control {
            let mut control = lock(control);
            // Whatever the controller was holding down is let go (U-9).
            if control.controller() == Some(id) {
                self.input.release_all();
            }
            control.disconnected(id);
        } else if !self.settings.view_only {
            // A live desktop: a viewer that drops mid-keypress mustn't leave
            // Ctrl or Super held on the user's own session.
            self.input.release_all();
        }
        lock(&self.tasks).remove(&id);
        let left = lock(&self.viewers).remove(&id);
        if let Some(viewer) = left {
            self.update_presence().await;
            notify::announce("A viewer disconnected", &viewer).await;
        }
    }

    fn session_config(&self, peer: SocketAddr) -> ServerSessionConfig {
        let loopback = peer.ip().is_loopback() || is_mapped_loopback(peer.ip());
        let mut config = ServerSessionConfig::new(peer.to_string(), Arc::new(OsEntropy));
        config.loopback = loopback;
        config.allow_legacy = self.settings.allow_legacy;
        config.tls_available = true;
        config.ra2_key = Some(Arc::clone(&self.settings.ra2));
        let logins = lock(&self.logins).clone();
        config.user_verifier.clone_from(&logins.verifier);
        config.vnc_password.clone_from(&logins.vnc_password);
        config.owner.clone_from(&self.settings.owner);
        config.view_only = self.settings.view_only;
        config.prompt_non_owner = !self.settings.headless;
        // A view-only viewer doesn't get the desktop's clipboard either.
        config.send_clipboard = !self.settings.view_only;
        // Legacy types as documented (ADR-0018, P-26); `SecurityPolicy` still
        // drops them off-loopback unless --allow-legacy.
        if logins.vnc_password.is_some() {
            config.security_types.push(SecurityType::VncAuth);
        }
        // Never on a headless desktop: nobody is there to approve (it also
        // refuses to start without a password).
        if loopback && !self.settings.headless && !logins.any() {
            config.security_types.push(SecurityType::None);
        }
        config
    }

    async fn run_session(
        self: Arc<Self>,
        id: ConnectionId,
        stream: TcpStream,
        peer: SocketAddr,
    ) -> Result<()> {
        let owner = Arc::new(AtomicBool::new(false));
        let gate = Gate {
            agent: Arc::clone(&self),
            id,
            owner: Arc::clone(&owner),
        };
        let resizer = match &self.input {
            Input::Headless(handle) if self.settings.headless => Some(Box::new(ResizeBridge {
                handle: handle.clone(),
                agent: Arc::clone(&self),
                gate: gate.clone(),
            })
                as Box<dyn DesktopResizer>),
            _ => None,
        };
        let ports = SessionPorts {
            input: Box::new(InputBridge(self.input.clone(), gate.clone())),
            clipboard: Box::new(ClipboardBridge(self.input.clone(), gate)),
            resizer,
        };
        let mut session = ServerSession::new(self.session_config(peer), ports);
        let mut transport = Transport::new(stream);
        let mut desktop_changes = self.version.subscribe();
        let mut clipboard = self.clipboard.subscribe();
        let (approval_tx, mut approval) = mpsc::channel::<bool>(1);
        let deadline = tokio::time::Instant::now() + HANDSHAKE_DEADLINE;
        let mut buf = vec![0u8; 64 * 1024];
        write(&mut transport, &session.take_output()).await?;
        loop {
            // The deadline applies until the viewer is in or waiting for a decision.
            let handshaking = !session.is_connected() && !session.awaiting_decision();
            let events = tokio::select! {
                read = transport.read(&mut buf) => {
                    let Some(bytes) = read? else { return Ok(()) };
                    session.receive(&bytes);
                    self.poll(&mut session, &mut transport, peer.ip(), deadline).await?
                }
                () = tokio::time::sleep_until(deadline), if handshaking => {
                    return Err(anyhow!("the viewer did not finish logging in within {}s", HANDSHAKE_DEADLINE.as_secs()));
                }
                changed = desktop_changes.changed(), if session.is_connected() => {
                    changed?;
                    session.frame_ready(&lock(&self.desktop))?;
                    Vec::new()
                }
                text = clipboard.recv(), if session.is_connected() => {
                    if let Ok(text) = text
                        && self.receives_clipboard(id, owner.load(Ordering::Acquire))
                    {
                        session.send_clipboard(&text)?;
                    }
                    Vec::new()
                }
                Some(allowed) = approval.recv() => {
                    if allowed {
                        session.allow(&lock(&self.desktop))?;
                        session.poll()?
                    } else {
                        // A denied or unanswered prompt counts as a failed login.
                        lock(&self.access).auth_failed(peer.ip(), self.now_ms());
                        session.deny();
                        write(&mut transport, &session.take_output()).await?;
                        return Ok(());
                    }
                }
            };
            for event in events {
                if let SessionEvent::Connected { owner: true, .. } = &event {
                    owner.store(true, Ordering::Release);
                }
                self.on_event(id, &mut session, event, &approval_tx, peer.ip())
                    .await;
            }
            // A viewer's update request may be answerable right away.
            if session.is_connected() {
                session.frame_ready(&lock(&self.desktop))?;
            }
            write(&mut transport, &session.take_output()).await?;
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
        deadline: tokio::time::Instant,
    ) -> Result<Vec<SessionEvent>> {
        let mut out = Vec::new();
        loop {
            // Password checks (Argon2id) are deliberately slow: keep them off
            // the async workers while logging in.
            let polled = if session.is_connected() {
                session.poll()
            } else {
                tokio::task::block_in_place(|| session.poll())
            };
            let events = match polled {
                Ok(events) => events,
                Err(ProtocolError::Auth(error)) => {
                    lock(&self.access).auth_failed(ip, self.now_ms());
                    write(transport, &session.take_output()).await?;
                    return Err(error.into());
                }
                Err(error) => {
                    write(transport, &session.take_output()).await?;
                    return Err(error.into());
                }
            };
            let mut upgraded = false;
            for event in events {
                match event {
                    SessionEvent::UpgradeTransport(upgrade) => {
                        // Bytes queued before the upgrade go out in the clear.
                        write(transport, &session.take_output()).await?;
                        match upgrade {
                            TransportUpgrade::Tls => {
                                tokio::time::timeout_at(
                                    deadline,
                                    transport.accept_tls(&self.settings.tls),
                                )
                                .await
                                .context("the TLS handshake took too long")??;
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
        ip: IpAddr,
    ) {
        match event {
            SessionEvent::ClientInit { shared } => {
                let admission = lock(&self.access).admit(id, shared);
                match admission {
                    Admission::Admit { disconnect } => {
                        {
                            let tasks = lock(&self.tasks);
                            for other in disconnect {
                                if let Some(task) = tasks.get(&other) {
                                    task.abort();
                                }
                            }
                        }
                        if let Err(error) = session.admit(&lock(&self.desktop)) {
                            warn!(%error, "could not admit the viewer");
                            session.deny();
                        }
                    }
                    Admission::Refuse => session.deny(),
                }
            }
            SessionEvent::AccessRequest { username, peer } => {
                self.ask(session, username, &peer, approval, ip);
            }
            SessionEvent::Connected {
                username,
                owner,
                view_only,
                ..
            } => {
                // Only a viewer that is fully in clears its address's failures
                // and counts as admitted.
                {
                    let mut access = lock(&self.access);
                    access.auth_succeeded(ip);
                    access.started(id);
                }
                let mut label = username.unwrap_or_else(|| session.peer().to_owned());
                if owner {
                    label.push_str(" (you)");
                }
                if view_only {
                    label.push_str(", view only");
                }
                lock(&self.viewers).insert(id, label.clone());
                info!(viewer = %label, "viewer connected");
                self.update_presence().await;
                notify::announce("Someone is viewing your screen", &label).await;
            }
            SessionEvent::UpgradeTransport(_) | SessionEvent::Authenticated { .. } => {}
        }
    }

    /// On a shared headless desktop only the viewer in control (or the
    /// owner) gets the desktop's clipboard: copied secrets don't reach
    /// people who are just watching.
    fn receives_clipboard(&self, id: ConnectionId, owner: bool) -> bool {
        match &self.control {
            None => true,
            Some(control) => owner || lock(control).controller() == Some(id),
        }
    }

    /// Shows the approval prompt. Only one can be open at a time: a request
    /// while one is pending is refused and counted as a failure, so nobody can
    /// flood the person at the screen with prompts until one is accepted (T1621).
    fn ask(
        &self,
        session: &ServerSession,
        username: Option<String>,
        peer: &str,
        approval: &mpsc::Sender<bool>,
        ip: IpAddr,
    ) {
        if self.prompt_open.swap(true, Ordering::AcqRel) {
            lock(&self.access).auth_failed(ip, self.now_ms());
            let _ = approval.try_send(false);
            return;
        }
        let security = session
            .security()
            .map_or_else(|| "unknown".to_owned(), |s| format!("{s:?}"));
        let who = match username {
            Some(user) => format!("{user} at {peer} (signed in with {security})"),
            None => format!("Someone at {peer} using the shared password ({security})"),
        };
        let approval = approval.clone();
        let open = Arc::clone(&self.prompt_open);
        tokio::spawn(async move {
            let allowed = notify::ask_access(&who).await;
            open.store(false, Ordering::Release);
            let _ = approval.send(allowed).await;
        });
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

/// Lets a viewer's input through only while it has control (U-9). Live
/// desktops have no control rule, so everything goes through.
#[derive(Clone)]
struct Gate {
    agent: Arc<Agent>,
    id: ConnectionId,
    /// Set once the viewer is in, if it logged in as the owner.
    owner: Arc<AtomicBool>,
}

impl Gate {
    fn open(&self) -> bool {
        let Some(control) = &self.agent.control else {
            return true;
        };
        let role = if self.owner.load(Ordering::Acquire) {
            Role::Owner
        } else {
            Role::Guest
        };
        match lock(control).request(self.id, role, self.agent.now_ms()) {
            Decision::Allowed => true,
            Decision::TookOver { from } => {
                info!(viewer = ?self.id, previous = ?from, "viewer took control");
                if from.is_some() {
                    self.agent.input.release_all();
                }
                true
            }
            Decision::Refused => false,
        }
    }
}

struct InputBridge(Input, Gate);

impl InputSink for InputBridge {
    fn key(&mut self, down: bool, keysym: u32) {
        if self.1.open() {
            self.0.key(down, keysym);
        }
    }

    fn key_code(&mut self, down: bool, keysym: u32, evdev: u16) {
        if self.1.open() {
            self.0.key_code(down, keysym, evdev);
        }
    }

    fn pointer(&mut self, buttons: u8, x: u16, y: u16) {
        if self.1.open() {
            self.0.pointer(buttons, x, y);
        }
    }
}

struct ClipboardBridge(Input, Gate);

impl ClipboardSink for ClipboardBridge {
    fn set_text(&mut self, text: &str) {
        if self.1.open() {
            self.0.set_clipboard(text);
        }
    }
}

/// Resizes a headless desktop when a viewer asks (`SetDesktopSize`).
struct ResizeBridge {
    handle: farsight_headless::Handle,
    agent: Arc<Agent>,
    /// Only the viewer in control may reshape the shared desktop (U-9).
    gate: Gate,
}

impl DesktopResizer for ResizeBridge {
    fn resize(&mut self, width: u16, height: u16, screens: &[Screen]) -> ResizeOutcome {
        if !self.gate.open() {
            return ResizeOutcome::Prohibited;
        }
        let now = self.agent.now_ms();
        {
            let mut last = lock(&self.agent.last_resize_ms);
            if last.is_some_and(|at| now.saturating_sub(at) < MIN_RESIZE_INTERVAL_MS) {
                return ResizeOutcome::OutOfResources;
            }
            *last = Some(now);
        }
        // The compositor answers within milliseconds (two seconds at most).
        let result = tokio::task::block_in_place(|| self.handle.resize(width, height, screens));
        match result {
            farsight_headless::ResizeResult::Resized { generation } => {
                self.agent
                    .frame_generation
                    .fetch_max(generation, Ordering::AcqRel);
                lock(&self.agent.desktop).resize_with_screens(width, height, screens);
                self.agent.version.send_modify(|v| *v = v.wrapping_add(1));
                ResizeOutcome::Accepted
            }
            farsight_headless::ResizeResult::InvalidLayout => ResizeOutcome::InvalidLayout,
            farsight_headless::ResizeResult::Failed => ResizeOutcome::OutOfResources,
        }
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
