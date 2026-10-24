//! One viewer connection on the agent: security offer, owner check and
//! access prompt (U-2), input and clipboard routing, and updates.
//!
//! Lifecycle, driven by the adapter:
//!
//! 1. [`ServerSession::new`], then shuttle bytes with
//!    [`ServerSession::receive`] / [`ServerSession::take_output`] and call
//!    [`ServerSession::poll`] after every receive.
//! 2. [`SessionEvent::UpgradeTransport`]: wrap the socket in TLS or RA2
//!    framing, then [`ServerSession::transport_upgraded`].
//! 3. [`SessionEvent::ClientInit`]: consult [`crate::AccessControl::admit`],
//!    then [`ServerSession::admit`] or [`ServerSession::deny`].
//! 4. [`SessionEvent::AccessRequest`] (viewer is not the owner): ask the
//!    person at the screen, then [`ServerSession::allow`] or
//!    [`ServerSession::deny`]. Timing out is the adapter's job (deny).
//! 5. [`SessionEvent::Connected`]: from now on call
//!    [`ServerSession::frame_ready`] whenever the [`Desktop`] changed and
//!    after every `poll`.
//!
//! A `poll` error of [`ProtocolError::Auth`] is a failed login: report it
//! with [`crate::AccessControl::auth_failed`]. After any error, flush the
//! output (it may carry the failure reason) and close the connection.

use std::fmt;
use std::sync::{Arc, OnceLock};

use farsight_rfb::auth::TransportUpgrade;
use farsight_rfb::handshake::ServerInit;
use farsight_rfb::server::{ServerConfig, ServerConnection, ServerEvent};
use farsight_rfb::{
    ClientMessage, CutText, DecodeError, Encoding, Limits, PixelFormat, ProtocolError,
    ProtocolVersion, SecurityType, ServerMessage,
};
use farsight_security::ra2::Ra2ServerKey;
use secrecy::SecretString;

use crate::damage::DEFAULT_MAX_RECTS;
use crate::ports::{
    ClipboardSink, DesktopResizer, Entropy, InputSink, ResizeOutcome, UserVerifier,
};
use crate::update::UpdateState;
use crate::{Desktop, security};

/// Largest clipboard text accepted from or sent to a viewer by default.
pub const DEFAULT_MAX_CLIPBOARD: u32 = 1024 * 1024;

/// How one [`ServerSession`] behaves. Build with [`Self::new`] and adjust
/// the fields.
#[allow(clippy::struct_excessive_bools)] // Independent settings.
#[derive(Clone)]
pub struct ServerSessionConfig {
    /// The viewer's address, shown in the access prompt and logs.
    pub peer: String,
    /// The connection is over loopback: legacy security types are allowed.
    pub loopback: bool,
    /// Allow legacy security types (None, VncAuth) off loopback too. Off by
    /// default (ADR-0007, ADR-0009); using them logs a warning.
    pub allow_legacy: bool,
    /// Security types to offer, most preferred first. Filtered by policy
    /// and by the material below.
    pub security_types: Vec<SecurityType>,
    /// The adapter can run TLS (has a certificate) for VeNCrypt.
    pub tls_available: bool,
    /// The agent's RSA key, enabling RA2 and RA2_256.
    pub ra2_key: Option<Arc<Ra2ServerKey>>,
    /// Username/password checker (VeNCrypt Plain, RA2 user logins).
    pub user_verifier: Option<Arc<dyn UserVerifier>>,
    /// Legacy VNC password (VncAuth, VeNCrypt `X509Vnc`, RA2 password-only).
    pub vnc_password: Option<SecretString>,
    /// Randomness for VncAuth challenges.
    pub entropy: Arc<dyn Entropy>,
    /// The OS user running the agent. A viewer that logs in with this
    /// username skips the access prompt (U-2).
    pub owner: Option<String>,
    /// Ask the person at the screen before admitting anyone else (U-2). Off
    /// only for desktops nobody sits at (M7 headless).
    pub prompt_non_owner: bool,
    /// Drop this viewer's input and clipboard (enforced here, API3).
    pub view_only: bool,
    /// Send desktop clipboard changes to this viewer.
    pub send_clipboard: bool,
    /// Pixel encodings the agent may use, most preferred first.
    pub encodings: Vec<Encoding>,
    /// Newest RFB version to offer.
    pub max_version: ProtocolVersion,
    /// Bounds on viewer-announced lengths.
    pub limits: Limits,
    /// Most damage rectangles kept per viewer before merging.
    pub max_damage_rects: usize,
    /// Longest clipboard text (in bytes) sent to the viewer; longer text is
    /// not sent at all rather than cut.
    pub max_clipboard_out: usize,
}

impl ServerSessionConfig {
    /// Secure defaults for a viewer at `peer`: encrypted types only
    /// (VeNCrypt, RA2_256, RA2), prompt for non-owners, Tight > ZRLE > Raw.
    #[must_use]
    pub fn new(peer: impl Into<String>, entropy: Arc<dyn Entropy>) -> Self {
        Self {
            peer: peer.into(),
            loopback: false,
            allow_legacy: false,
            security_types: vec![
                SecurityType::VeNCrypt,
                SecurityType::Ra256,
                SecurityType::Ra2,
            ],
            tls_available: false,
            ra2_key: None,
            user_verifier: None,
            vnc_password: None,
            entropy,
            owner: None,
            prompt_non_owner: true,
            view_only: false,
            send_clipboard: true,
            encodings: vec![Encoding::Tight, Encoding::Zrle, Encoding::Raw],
            max_version: ProtocolVersion::V3_8,
            limits: Limits {
                max_cut_text: DEFAULT_MAX_CLIPBOARD,
                ..Limits::DEFAULT
            },
            max_damage_rects: DEFAULT_MAX_RECTS,
            max_clipboard_out: DEFAULT_MAX_CLIPBOARD as usize,
        }
    }

    /// The security types that would be offered.
    #[must_use]
    pub fn offer(&self) -> Vec<SecurityType> {
        security::offer(self)
    }
}

impl fmt::Debug for ServerSessionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the password; keys and verifiers are opaque.
        f.debug_struct("ServerSessionConfig")
            .field("peer", &self.peer)
            .field("loopback", &self.loopback)
            .field("allow_legacy", &self.allow_legacy)
            .field("security_types", &self.security_types)
            .field("tls_available", &self.tls_available)
            .field("ra2_key", &self.ra2_key.is_some())
            .field("user_verifier", &self.user_verifier.is_some())
            .field("vnc_password", &self.vnc_password.is_some())
            .field("owner", &self.owner)
            .field("prompt_non_owner", &self.prompt_non_owner)
            .field("view_only", &self.view_only)
            .field("encodings", &self.encodings)
            .finish_non_exhaustive()
    }
}

/// The adapter-provided ports one session uses.
pub struct SessionPorts {
    /// Where keyboard and pointer input goes.
    pub input: Box<dyn InputSink>,
    /// Where clipboard text from the viewer goes.
    pub clipboard: Box<dyn ClipboardSink>,
    /// Resize handler; `None` refuses `SetDesktopSize` as prohibited.
    pub resizer: Option<Box<dyn DesktopResizer>>,
}

/// Something the adapter must act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Wrap the transport (TLS for VeNCrypt, AES-EAX framing for RA2),
    /// sending any output taken before this event in the clear, then call
    /// [`ServerSession::transport_upgraded`].
    UpgradeTransport(TransportUpgrade),
    /// The viewer authenticated (report it to
    /// [`crate::AccessControl::auth_succeeded`]).
    Authenticated {
        /// The security type used.
        security: SecurityType,
        /// The username that logged in, for types that carry one.
        username: Option<String>,
    },
    /// The viewer sent `ClientInit`; decide with
    /// [`crate::AccessControl::admit`], then call [`ServerSession::admit`]
    /// or [`ServerSession::deny`].
    ClientInit {
        /// The viewer is willing to share the desktop.
        shared: bool,
    },
    /// Ask the person at the screen whether to let this viewer in, then call
    /// [`ServerSession::allow`] or [`ServerSession::deny`].
    AccessRequest {
        /// The username that logged in, if the security type has one.
        username: Option<String>,
        /// The viewer's address.
        peer: String,
    },
    /// The viewer is in: `ServerInit` was sent.
    Connected {
        /// The security type used.
        security: SecurityType,
        /// The username that logged in, if any.
        username: Option<String>,
        /// The viewer logged in as the desktop's owner (no prompt shown).
        owner: bool,
        /// The viewer cannot send input.
        view_only: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Handshake,
    AwaitAdmission,
    AwaitApproval,
    Connected,
    Closed,
}

/// One viewer connection on the agent, independent of sockets and UI.
pub struct ServerSession {
    config: ServerSessionConfig,
    ports: SessionPorts,
    connection: ServerConnection,
    stage: Stage,
    security: Option<SecurityType>,
    verified_user: Arc<OnceLock<String>>,
    shared: bool,
    events: Vec<SessionEvent>,
    update: Option<UpdateState>,
    clipboard_serial: u64,
    last_clipboard_in: Option<String>,
    premature_input: bool,
}

impl ServerSession {
    /// Starts a session and queues the server's version message. The
    /// security offer is computed now from `config`.
    #[must_use]
    pub fn new(config: ServerSessionConfig, ports: SessionPorts) -> Self {
        let connection = ServerConnection::new(ServerConfig {
            max_version: config.max_version,
            security_types: security::offer(&config),
            limits: config.limits,
        });
        Self {
            config,
            ports,
            connection,
            stage: Stage::Handshake,
            premature_input: false,
            security: None,
            verified_user: Arc::new(OnceLock::new()),
            shared: false,
            events: Vec::new(),
            update: None,
            clipboard_serial: 0,
            last_clipboard_in: None,
        }
    }

    /// Appends bytes received from the viewer. A viewer must stay silent
    /// between `ClientInit` and `ServerInit`; anything sent while it waits
    /// for admission or approval (e.g. keystrokes queued to replay the moment
    /// the owner clicks Allow) closes the connection.
    pub fn receive(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() && self.awaiting_decision() {
            self.premature_input = true;
            return;
        }
        self.connection.receive(bytes);
    }

    /// Whether the viewer waits for the sharing check or the approval prompt.
    #[must_use]
    pub fn awaiting_decision(&self) -> bool {
        matches!(self.stage, Stage::AwaitAdmission | Stage::AwaitApproval)
    }

    /// Takes the bytes to send to the viewer.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.connection.take_output()
    }

    /// The negotiated security type, once chosen.
    #[must_use]
    pub fn security(&self) -> Option<SecurityType> {
        self.security
    }

    /// The username the viewer logged in with, if any.
    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.verified_user.get().map(String::as_str)
    }

    /// Whether the viewer logged in as the desktop's owner.
    #[must_use]
    pub fn is_owner(&self) -> bool {
        matches!((self.username(), &self.config.owner), (Some(user), Some(owner)) if user == owner)
    }

    /// The viewer's address.
    #[must_use]
    pub fn peer(&self) -> &str {
        &self.config.peer
    }

    /// Whether the viewer asked to share the desktop.
    #[must_use]
    pub fn shared(&self) -> bool {
        self.shared
    }

    /// Whether the viewer is in (`ServerInit` sent).
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.stage == Stage::Connected
    }

    /// Whether the session ended; close the connection after flushing output.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.stage == Stage::Closed
    }

    /// Whether input from this viewer is dropped.
    #[must_use]
    pub fn view_only(&self) -> bool {
        self.config.view_only
    }

    /// Turns view-only on or off (e.g. from the agent's tray menu).
    pub fn set_view_only(&mut self, view_only: bool) {
        self.config.view_only = view_only;
    }

    /// The pixel encoding chosen for this viewer (Raw until `SetEncodings`).
    #[must_use]
    pub fn encoding(&self) -> Encoding {
        self.update
            .as_ref()
            .map_or(Encoding::Raw, UpdateState::encoding)
    }

    /// Processes buffered viewer input: handshake steps, then messages
    /// (input and clipboard go to the ports). Returns what the adapter must
    /// act on.
    ///
    /// # Errors
    /// Any [`ProtocolError`] ([`ProtocolError::Auth`] for a failed login);
    /// the session is then closed.
    pub fn poll(&mut self) -> Result<Vec<SessionEvent>, ProtocolError> {
        let result = self.drain();
        if result.is_err() {
            self.stage = Stage::Closed;
        }
        result.map(|()| std::mem::take(&mut self.events))
    }

    fn drain(&mut self) -> Result<(), ProtocolError> {
        if matches!(self.stage, Stage::Closed) {
            return Ok(());
        }
        if self.premature_input {
            return Err(ProtocolError::Decode(DecodeError::Malformed(
                "viewer sent data before it was admitted",
            )));
        }
        while let Some(event) = self.connection.poll()? {
            match event {
                ServerEvent::SecuritySelected(security) => self.on_security_selected(security)?,
                ServerEvent::UpgradeTransport(upgrade) => {
                    self.events.push(SessionEvent::UpgradeTransport(upgrade));
                    return Ok(());
                }
                ServerEvent::ClientInit { shared } => {
                    self.on_client_init(shared);
                    return Ok(());
                }
                ServerEvent::Message(message) => self.on_message(message)?,
                ServerEvent::CutTextDiscarded { len } => {
                    tracing::debug!(len, "oversized clipboard text from viewer discarded");
                }
            }
        }
        Ok(())
    }

    fn on_security_selected(&mut self, security: SecurityType) -> Result<(), ProtocolError> {
        if security.is_legacy_insecure() {
            tracing::warn!(peer = %self.config.peer, ?security, "viewer is using an unencrypted legacy security type");
        }
        let auth = security::authenticator(&self.config, security, &self.verified_user)?;
        self.connection.start_authentication(security, auth)?;
        self.security = Some(security);
        Ok(())
    }

    fn on_client_init(&mut self, shared: bool) {
        self.shared = shared;
        self.stage = Stage::AwaitAdmission;
        self.events.push(SessionEvent::Authenticated {
            security: self.security.unwrap_or(SecurityType::None),
            username: self.username().map(str::to_owned),
        });
        self.events.push(SessionEvent::ClientInit { shared });
    }

    /// Reports that the transport upgrade asked for with
    /// [`SessionEvent::UpgradeTransport`] is in place.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no upgrade was requested.
    pub fn transport_upgraded(&mut self) -> Result<(), ProtocolError> {
        self.connection.transport_upgraded()
    }

    /// Lets the viewer past the sharing check. Either it connects now (the
    /// owner, or no prompt configured) or [`SessionEvent::AccessRequest`]
    /// follows from the next [`Self::poll`].
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] unless [`SessionEvent::ClientInit`]
    /// is pending.
    pub fn admit(&mut self, desktop: &Desktop) -> Result<(), ProtocolError> {
        if self.stage != Stage::AwaitAdmission {
            return Err(ProtocolError::InvalidState(
                "no ClientInit awaiting admission",
            ));
        }
        if self.config.prompt_non_owner && !self.is_owner() {
            self.stage = Stage::AwaitApproval;
            self.events.push(SessionEvent::AccessRequest {
                username: self.username().map(str::to_owned),
                peer: self.config.peer.clone(),
            });
            Ok(())
        } else {
            self.start(desktop)
        }
    }

    /// The person at the screen allowed the viewer in.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] unless an access request is pending.
    pub fn allow(&mut self, desktop: &Desktop) -> Result<(), ProtocolError> {
        if self.stage != Stage::AwaitApproval {
            return Err(ProtocolError::InvalidState("no access request pending"));
        }
        self.start(desktop)
    }

    /// Refuses the viewer (sharing conflict, prompt denied or timed out) or
    /// ends the session. Close the connection afterwards.
    pub fn deny(&mut self) {
        if self.stage != Stage::Closed {
            tracing::info!(peer = %self.config.peer, "viewer refused or disconnected");
        }
        self.stage = Stage::Closed;
    }

    fn start(&mut self, desktop: &Desktop) -> Result<(), ProtocolError> {
        let fb = desktop.framebuffer();
        self.connection.send_server_init(&ServerInit {
            width: fb.width(),
            height: fb.height(),
            pixel_format: PixelFormat::RGB888,
            name: desktop.name().to_owned(),
        })?;
        self.stage = Stage::Connected;
        self.update = Some(UpdateState::new(
            self.config.encodings.clone(),
            self.config.max_damage_rects,
            desktop,
        ));
        self.clipboard_serial = desktop.clipboard_serial();
        let owner = self.is_owner();
        tracing::info!(peer = %self.config.peer, owner, view_only = self.config.view_only, "viewer connected");
        self.events.push(SessionEvent::Connected {
            security: self.security.unwrap_or(SecurityType::None),
            username: self.username().map(str::to_owned),
            owner,
            view_only: self.config.view_only,
        });
        Ok(())
    }

    fn on_message(&mut self, message: ClientMessage) -> Result<(), ProtocolError> {
        let Some(update) = self.update.as_mut() else {
            return Err(ProtocolError::InvalidState("message before ServerInit"));
        };
        let view_only = self.config.view_only;
        match message {
            ClientMessage::SetPixelFormat(format) => {
                update.set_pixel_format(format, &mut self.connection)?;
            }
            ClientMessage::SetEncodings(list) => update.set_encodings(&list),
            ClientMessage::FramebufferUpdateRequest { incremental, rect } => {
                update.request(incremental, rect);
            }
            ClientMessage::KeyEvent { down, keysym } if !view_only => {
                self.ports.input.key(down, keysym);
            }
            ClientMessage::PointerEvent { buttons, x, y } if !view_only => {
                let (width, height) = update.client_size();
                let x = x.min(width.saturating_sub(1));
                let y = y.min(height.saturating_sub(1));
                self.ports.input.pointer(buttons, x, y);
            }
            ClientMessage::ClientCutText(CutText::Text(bytes)) if !view_only => {
                let text = CutText::latin1_to_string(&bytes);
                self.ports.clipboard.set_text(&text);
                self.last_clipboard_in = Some(text);
            }
            ClientMessage::SetDesktopSize {
                width,
                height,
                screens,
            } if update.supports_extended_resize() => {
                let outcome = match (&mut self.ports.resizer, view_only) {
                    (Some(resizer), false) => resizer.resize(width, height, &screens),
                    _ => ResizeOutcome::Prohibited,
                };
                update.reply_to_resize(outcome.status_code());
            }
            // View-only input, and SetDesktopSize without negotiation.
            _ => {}
        }
        Ok(())
    }

    /// Sends what the viewer is owed from `desktop`: an update when one is
    /// requested and something changed (or the request was non-incremental),
    /// and clipboard changes. Call after every [`Desktop`] change and after
    /// every [`Self::poll`]. Does nothing until connected.
    ///
    /// # Errors
    /// [`ProtocolError`] if encoding fails (a bug, not viewer input).
    pub fn frame_ready(&mut self, desktop: &Desktop) -> Result<(), ProtocolError> {
        if self.stage != Stage::Connected {
            return Ok(());
        }
        if desktop.clipboard_serial() != self.clipboard_serial {
            self.clipboard_serial = desktop.clipboard_serial();
            if let Some(text) = desktop.clipboard() {
                self.send_clipboard(text)?;
            }
        }
        let Some(update) = self.update.as_mut() else {
            return Ok(());
        };
        update.sync(desktop);
        update.flush(desktop, &mut self.connection)
    }

    /// Sends clipboard text to the viewer (Latin-1; characters outside it
    /// become `?`). Skipped when disabled, too long, not yet connected, or
    /// equal to what this viewer itself just sent.
    ///
    /// # Errors
    /// [`ProtocolError`] if the message cannot be encoded.
    pub fn send_clipboard(&mut self, text: &str) -> Result<(), ProtocolError> {
        if self.stage != Stage::Connected
            || !self.config.send_clipboard
            || self.last_clipboard_in.as_deref() == Some(text)
        {
            return Ok(());
        }
        let bytes = CutText::string_to_latin1(text);
        if bytes.len() > self.config.max_clipboard_out {
            tracing::debug!(len = bytes.len(), "clipboard text too long to send");
            return Ok(());
        }
        self.connection
            .send(&ServerMessage::ServerCutText(CutText::Text(bytes)))
    }
}
