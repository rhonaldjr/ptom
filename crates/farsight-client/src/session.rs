//! One viewer connection: handshake policy, update loop, input and clipboard.

use farsight_codec::{Decoder, Framebuffer};
use farsight_rfb::auth::{AuthRequest, AuthResponse, Authenticator, NoAuth, TransportUpgrade};
use farsight_rfb::client::{ClientConfig, ClientConnection, ClientEvent};
use farsight_rfb::{
    ClientMessage, CutText, Encoding, Limits, PixelFormat, ProtocolError, Rect, RectHeader, Screen,
    SecurityType,
};
use farsight_security::SecurityPolicy;
use farsight_security::ra2::{Ra2Client, Ra2Variant};
use farsight_security::vencrypt::VeNCryptClient;
use farsight_security::vnc_auth::VncAuthClient;
use secrecy::SecretString;

/// Security types this build can complete, strongest first.
const IMPLEMENTED_SECURITY: [SecurityType; 5] = [
    SecurityType::VeNCrypt,
    SecurityType::Ra256,
    SecurityType::Ra2,
    SecurityType::VncAuth,
    SecurityType::None,
];

/// SHA-256 fingerprint of a server identity (TLS leaf certificate or RA2 key).
pub type Fingerprint = [u8; 32];

/// What kind of key a fingerprint identifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    /// A TLS server certificate (VeNCrypt).
    Certificate,
    /// An RSA public key (RSA-AES).
    RsaKey,
}

/// How a [`Session`] should connect.
#[allow(clippy::struct_excessive_bools)] // Independent user settings.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// The server address resolved to a loopback IP (legacy security allowed).
    pub loopback: bool,
    /// The user already agreed to an unencrypted connection to this server.
    pub allow_insecure: bool,
    /// Username, if already known (for Plain / RSA-AES user logins).
    pub username: Option<SecretString>,
    /// Password, if already known (from the vault or a password file).
    pub password: Option<SecretString>,
    /// Fingerprint the user trusted before for this server.
    pub pinned_identity: Option<Fingerprint>,
    /// This server used encryption before: refuse unencrypted security
    /// rather than offer it (downgrade protection).
    pub expect_encryption: bool,
    /// Do not send input.
    pub view_only: bool,
    /// Ask the server to keep other viewers connected.
    pub shared: bool,
    /// JPEG quality 0–9.
    pub quality: Option<u8>,
    /// Compression level 0–9.
    pub compress: Option<u8>,
}

/// Where the session is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Negotiating with the server.
    Handshaking,
    /// Paused until [`Session::provide_credentials`].
    WaitingForCredentials,
    /// Paused until [`Session::confirm_identity`].
    WaitingForIdentity,
    /// Paused until [`Session::consent_insecure`].
    WaitingForInsecureConsent,
    /// Desktop is live.
    Connected,
    /// The connection failed or was closed.
    Closed,
}

/// Something the UI should react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Ask the user for a password (and a username if `username`), then call
    /// [`Session::provide_credentials`].
    CredentialsRequired {
        /// Whether a username is needed too.
        username: bool,
    },
    /// The server's identity is new or has changed. Show the fingerprint and
    /// call [`Session::confirm_identity`] only if the user trusts it. No
    /// credentials have been sent yet.
    IdentityConfirmationRequired {
        /// The presented fingerprint.
        fingerprint: Fingerprint,
        /// Certificate or RSA key.
        kind: IdentityKind,
        /// A different identity was pinned for this server before.
        changed: bool,
    },
    /// For the transport adapter: switch the connection to TLS or RA2
    /// framing, then call [`Session::tls_established`] (TLS) or
    /// [`Session::transport_upgraded`] (RA2).
    TransportUpgrade(TransportUpgrade),
    /// The server only offers unencrypted security. Warn the user and call
    /// [`Session::consent_insecure`] if they accept (Nielsen #5).
    InsecureConsentRequired {
        /// The legacy type that would be used.
        security: SecurityType,
    },
    /// The desktop is live.
    Connected {
        /// Desktop name.
        name: String,
        /// Framebuffer width.
        width: u16,
        /// Framebuffer height.
        height: u16,
        /// Security type in use.
        security: SecurityType,
        /// Whether traffic is encrypted.
        encrypted: bool,
        /// The server identity presented (and trusted) during the handshake.
        identity: Option<(IdentityKind, Fingerprint)>,
    },
    /// Part of the framebuffer changed.
    Damaged(Rect),
    /// The framebuffer was resized.
    Resized {
        /// New width.
        width: u16,
        /// New height.
        height: u16,
    },
    /// A full update finished.
    UpdateComplete,
    /// The cursor shape changed.
    CursorChanged,
    /// Ring the bell.
    Bell,
    /// Clipboard text from the server.
    Clipboard(String),
}

enum Pending {
    /// A legacy VncAuth password, needed before choosing the type.
    Password(SecurityType),
    /// Credentials requested by the running authenticator.
    Credentials,
    InsecureConsent(SecurityType),
    ServerKey,
    TlsIdentity,
}

/// One viewer connection, independent of sockets and UI.
pub struct Session {
    config: SessionConfig,
    connection: ClientConnection,
    decoder: Decoder,
    status: SessionStatus,
    pending: Option<Pending>,
    security: Option<SecurityType>,
    encrypted: bool,
    presented_identity: Option<(IdentityKind, Fingerprint)>,
    last_clipboard: Option<String>,
    /// Bytes that arrived over TLS while the user was still checking the
    /// server's identity. They are protected, but the connection hasn't been
    /// told about the upgrade yet, so they wait here until it has.
    held_after_upgrade: Vec<u8>,
}

/// Most bytes held back while waiting for an identity decision (the server
/// has nothing to say yet beyond a challenge, so this is generous).
const MAX_HELD_AFTER_UPGRADE: usize = 64 * 1024;

impl Session {
    /// Starts a session; feed it the server's bytes.
    #[must_use]
    pub fn new(config: SessionConfig) -> Self {
        let client_config = ClientConfig {
            shared: config.shared,
            ..ClientConfig::default()
        };
        Self {
            config,
            connection: ClientConnection::new(client_config),
            decoder: Decoder::new(0, 0, PixelFormat::RGB888, Limits::DEFAULT),
            status: SessionStatus::Handshaking,
            pending: None,
            security: None,
            encrypted: false,
            presented_identity: None,
            last_clipboard: None,
            held_after_upgrade: Vec::new(),
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        self.status
    }

    /// Whether traffic is encrypted (TLS or RA2 is in place).
    #[must_use]
    pub fn encrypted(&self) -> bool {
        self.encrypted
    }

    /// The server identity presented during this handshake, for pinning.
    #[must_use]
    pub fn presented_identity(&self) -> Option<(IdentityKind, Fingerprint)> {
        self.presented_identity
    }

    /// The negotiated security type, once chosen.
    #[must_use]
    pub fn security(&self) -> Option<SecurityType> {
        self.security
    }

    /// The remote screen.
    #[must_use]
    pub fn framebuffer(&self) -> &Framebuffer {
        self.decoder.framebuffer()
    }

    /// The rectangle decoder (cursor shape, screen layout).
    #[must_use]
    pub fn decoder(&self) -> &Decoder {
        &self.decoder
    }

    /// Whether input is suppressed.
    #[must_use]
    pub fn view_only(&self) -> bool {
        self.config.view_only
    }

    /// Turns view-only mode on or off.
    pub fn set_view_only(&mut self, view_only: bool) {
        self.config.view_only = view_only;
    }

    /// Appends bytes received from the server.
    pub fn receive(&mut self, bytes: &[u8]) {
        if matches!(self.pending, Some(Pending::TlsIdentity)) {
            // Over the cap the extra bytes are dropped, so the handshake fails
            // safely rather than buffering without limit.
            let room = MAX_HELD_AFTER_UPGRADE.saturating_sub(self.held_after_upgrade.len());
            self.held_after_upgrade.extend(bytes.iter().take(room));
            return;
        }
        self.connection.receive(bytes);
    }

    /// Bytes to send to the server.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.connection.take_output()
    }

    /// Processes buffered input and returns the resulting events.
    ///
    /// # Errors
    /// Any [`ProtocolError`]; the session is then closed.
    pub fn poll(&mut self) -> Result<Vec<SessionEvent>, ProtocolError> {
        let mut events = Vec::new();
        let result = self.drain(&mut events);
        if result.is_err() {
            self.status = SessionStatus::Closed;
        }
        result.map(|()| events)
    }

    /// Supplies the credentials requested by [`SessionEvent::CredentialsRequired`].
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if none were requested.
    pub fn provide_credentials(
        &mut self,
        username: Option<SecretString>,
        password: SecretString,
    ) -> Result<(), ProtocolError> {
        match self.pending.take() {
            Some(Pending::Password(security)) => {
                self.config.password = Some(password);
                self.choose(security)
            }
            Some(Pending::Credentials) => {
                self.status = SessionStatus::Handshaking;
                self.connection
                    .respond_auth(AuthResponse::Credentials { username, password })
            }
            other => {
                self.pending = other;
                Err(ProtocolError::InvalidState("no credentials requested"))
            }
        }
    }

    /// Supplies just a password (see [`Self::provide_credentials`]).
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if none was requested.
    pub fn provide_password(&mut self, password: SecretString) -> Result<(), ProtocolError> {
        self.provide_credentials(None, password)
    }

    /// Trusts the identity shown with [`SessionEvent::IdentityConfirmationRequired`].
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no confirmation was requested.
    pub fn confirm_identity(&mut self) -> Result<(), ProtocolError> {
        match self.pending.take() {
            Some(Pending::ServerKey) => {
                self.status = SessionStatus::Handshaking;
                self.connection
                    .respond_auth(AuthResponse::ServerKeyAccepted)
            }
            Some(Pending::TlsIdentity) => {
                self.status = SessionStatus::Handshaking;
                self.encrypted = true;
                self.connection.transport_upgraded()?;
                let held = std::mem::take(&mut self.held_after_upgrade);
                self.connection.receive(&held);
                Ok(())
            }
            other => {
                self.pending = other;
                Err(ProtocolError::InvalidState(
                    "no identity confirmation requested",
                ))
            }
        }
    }

    /// Called by the transport once TLS is up: `valid_chain` says whether the
    /// certificate chains to a trusted CA for this host name.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no TLS upgrade was requested.
    pub fn tls_established(
        &mut self,
        fingerprint: Fingerprint,
        valid_chain: bool,
    ) -> Result<Option<SessionEvent>, ProtocolError> {
        self.presented_identity = Some((IdentityKind::Certificate, fingerprint));
        if valid_chain || self.config.pinned_identity == Some(fingerprint) {
            self.encrypted = true;
            self.connection.transport_upgraded()?;
            return Ok(None);
        }
        self.pending = Some(Pending::TlsIdentity);
        self.status = SessionStatus::WaitingForIdentity;
        Ok(Some(SessionEvent::IdentityConfirmationRequired {
            fingerprint,
            kind: IdentityKind::Certificate,
            changed: self.config.pinned_identity.is_some(),
        }))
    }

    /// Called by the transport once RA2 framing is in place.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no upgrade was requested.
    pub fn transport_upgraded(&mut self) -> Result<(), ProtocolError> {
        self.encrypted = true;
        self.connection.transport_upgraded()
    }

    /// Accepts an unencrypted connection after [`SessionEvent::InsecureConsentRequired`].
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no consent was requested.
    pub fn consent_insecure(&mut self) -> Result<Option<SessionEvent>, ProtocolError> {
        let Some(Pending::InsecureConsent(security)) = self.pending.take() else {
            return Err(ProtocolError::InvalidState("no consent requested"));
        };
        self.config.allow_insecure = true;
        self.proceed_with(security)
    }

    /// Sends a key press or release (ignored in view-only mode).
    pub fn key(&mut self, down: bool, keysym: u32) {
        self.send_input(&ClientMessage::KeyEvent { down, keysym });
    }

    /// Sends pointer state, clamped to the framebuffer (ignored in view-only mode).
    pub fn pointer(&mut self, buttons: u8, x: u16, y: u16) {
        let fb = self.decoder.framebuffer();
        let x = x.min(fb.width().saturating_sub(1));
        let y = y.min(fb.height().saturating_sub(1));
        self.send_input(&ClientMessage::PointerEvent { buttons, x, y });
    }

    /// Asks the server to resize the desktop to `width` × `height` (one
    /// screen). Does nothing unless the server supports extended resizing,
    /// the size actually differs, and the session is not view-only.
    /// Returns whether a request was sent.
    pub fn request_resize(&mut self, width: u16, height: u16) -> bool {
        let fb = self.decoder.framebuffer();
        let unchanged = (fb.width(), fb.height()) == (width, height);
        if !self.decoder.supports_extended_resize() || unchanged || width == 0 || height == 0 {
            return false;
        }
        let id = self.decoder.screens().first().map_or(0, |s| s.id);
        let screen = Screen {
            id,
            x: 0,
            y: 0,
            width,
            height,
            flags: 0,
        };
        let message = ClientMessage::SetDesktopSize {
            width,
            height,
            screens: vec![screen],
        };
        let sent = self.status == SessionStatus::Connected && !self.config.view_only;
        self.send_input(&message);
        sent
    }

    /// Sends local clipboard text if it changed (ignored in view-only mode).
    pub fn clipboard(&mut self, text: &str) {
        if self.last_clipboard.as_deref() == Some(text) {
            return;
        }
        self.last_clipboard = Some(text.to_owned());
        self.send_input(&ClientMessage::ClientCutText(CutText::Text(
            CutText::string_to_latin1(text),
        )));
    }

    fn send_input(&mut self, message: &ClientMessage) {
        if self.status == SessionStatus::Connected && !self.config.view_only {
            // Sending only fails before the handshake, which the status check rules out.
            let _ = self.connection.send(message);
        }
    }

    fn drain(&mut self, events: &mut Vec<SessionEvent>) -> Result<(), ProtocolError> {
        while let Some(event) = self.connection.poll(&mut self.decoder)? {
            match event {
                ClientEvent::SecurityTypes(offered) => {
                    if let Some(event) = self.on_security_types(&offered)? {
                        events.push(event);
                        return Ok(());
                    }
                }
                ClientEvent::UpgradeTransport(upgrade) => {
                    events.push(SessionEvent::TransportUpgrade(upgrade));
                    return Ok(());
                }
                ClientEvent::AuthRequest(request) => {
                    if let Some(event) = self.on_auth_request(&request)? {
                        events.push(event);
                        return Ok(());
                    }
                }
                ClientEvent::Connected(init) => {
                    self.on_connected(init.width, init.height)?;
                    events.push(SessionEvent::Connected {
                        name: init.name,
                        width: init.width,
                        height: init.height,
                        security: self.security.unwrap_or(SecurityType::None),
                        encrypted: self.encrypted(),
                        identity: self.presented_identity,
                    });
                }
                ClientEvent::Rect(header) => events.push(self.on_rect(&header)),
                ClientEvent::UpdateComplete => {
                    self.request_update(true)?;
                    events.push(SessionEvent::UpdateComplete);
                }
                ClientEvent::ColourMap { first, colours } => {
                    self.decoder.set_colour_map(first, &colours);
                }
                ClientEvent::Bell => events.push(SessionEvent::Bell),
                ClientEvent::ServerCutText(text) => {
                    let text = CutText::latin1_to_string(&text);
                    self.last_clipboard = Some(text.clone());
                    events.push(SessionEvent::Clipboard(text));
                }
                ClientEvent::CutTextDiscarded { .. } => {}
            }
        }
        Ok(())
    }

    fn on_auth_request(
        &mut self,
        request: &AuthRequest,
    ) -> Result<Option<SessionEvent>, ProtocolError> {
        match *request {
            AuthRequest::Credentials { username } => {
                self.pending = Some(Pending::Credentials);
                self.status = SessionStatus::WaitingForCredentials;
                Ok(Some(SessionEvent::CredentialsRequired { username }))
            }
            AuthRequest::ConfirmServerKey { fingerprint } => {
                self.presented_identity = Some((IdentityKind::RsaKey, fingerprint));
                if self.config.pinned_identity == Some(fingerprint) {
                    self.connection
                        .respond_auth(AuthResponse::ServerKeyAccepted)?;
                    return Ok(None);
                }
                self.pending = Some(Pending::ServerKey);
                self.status = SessionStatus::WaitingForIdentity;
                Ok(Some(SessionEvent::IdentityConfirmationRequired {
                    fingerprint,
                    kind: IdentityKind::RsaKey,
                    changed: self.config.pinned_identity.is_some(),
                }))
            }
        }
    }

    fn on_security_types(
        &mut self,
        offered: &[SecurityType],
    ) -> Result<Option<SessionEvent>, ProtocolError> {
        let usable: Vec<_> = offered
            .iter()
            .copied()
            .filter(|t| IMPLEMENTED_SECURITY.contains(t))
            .collect();
        if self.config.expect_encryption && SecurityPolicy::NETWORK.select(&usable).is_none() {
            return Err(ProtocolError::Refused(
                "this computer used an encrypted connection before but now offers only unencrypted ones; \
                 someone may be interfering with the connection"
                    .into(),
            ));
        }
        let lenient = self.config.loopback || self.config.allow_insecure;
        let policy = if lenient {
            SecurityPolicy::LOOPBACK
        } else {
            SecurityPolicy::NETWORK
        };
        if let Some(security) = policy.select(&usable) {
            return self.proceed_with(security);
        }
        match SecurityPolicy::LOOPBACK.select(&usable) {
            Some(security) => {
                self.pending = Some(Pending::InsecureConsent(security));
                self.status = SessionStatus::WaitingForInsecureConsent;
                Ok(Some(SessionEvent::InsecureConsentRequired { security }))
            }
            None => Err(ProtocolError::Refused(format!(
                "the server offers no security type this viewer supports ({offered:?})"
            ))),
        }
    }

    fn proceed_with(
        &mut self,
        security: SecurityType,
    ) -> Result<Option<SessionEvent>, ProtocolError> {
        if security == SecurityType::VncAuth && self.config.password.is_none() {
            self.pending = Some(Pending::Password(security));
            self.status = SessionStatus::WaitingForCredentials;
            return Ok(Some(SessionEvent::CredentialsRequired { username: false }));
        }
        self.choose(security)?;
        Ok(None)
    }

    fn choose(&mut self, security: SecurityType) -> Result<(), ProtocolError> {
        let (username, password) = (self.config.username.clone(), self.config.password.clone());
        let auth: Box<dyn Authenticator> = match security {
            SecurityType::VncAuth => match password {
                Some(password) => Box::new(VncAuthClient::new(password)),
                None => return Err(ProtocolError::InvalidState("VncAuth without a password")),
            },
            SecurityType::VeNCrypt => Box::new(VeNCryptClient::new(username, password)),
            SecurityType::Ra2 | SecurityType::Ra256 => {
                let variant = Ra2Variant::for_security_type(security)
                    .ok_or(ProtocolError::InvalidState("unsupported RSA-AES variant"))?;
                Box::new(Ra2Client::new(variant, username, password))
            }
            _ => Box::new(NoAuth),
        };
        self.connection.choose_security(security, auth)?;
        self.security = Some(security);
        self.status = SessionStatus::Handshaking;
        Ok(())
    }

    /// Sets our pixel format and encodings, then asks for the first full update.
    fn on_connected(&mut self, width: u16, height: u16) -> Result<(), ProtocolError> {
        self.status = SessionStatus::Connected;
        self.decoder = Decoder::new(width, height, PixelFormat::RGB888, Limits::DEFAULT);
        self.connection
            .send(&ClientMessage::SetPixelFormat(PixelFormat::RGB888))?;
        self.connection
            .send(&ClientMessage::SetEncodings(self.encodings()))?;
        self.request_update(false)
    }

    fn on_rect(&mut self, header: &RectHeader) -> SessionEvent {
        match header.encoding {
            Encoding::DesktopSize | Encoding::ExtendedDesktopSize => {
                let fb = self.decoder.framebuffer();
                SessionEvent::Resized {
                    width: fb.width(),
                    height: fb.height(),
                }
            }
            Encoding::Cursor | Encoding::CursorWithAlpha => SessionEvent::CursorChanged,
            _ => SessionEvent::Damaged(header.rect),
        }
    }

    fn request_update(&mut self, incremental: bool) -> Result<(), ProtocolError> {
        let fb = self.decoder.framebuffer();
        let rect = Rect::new(0, 0, fb.width(), fb.height());
        self.connection
            .send(&ClientMessage::FramebufferUpdateRequest { incremental, rect })
    }

    fn encodings(&self) -> Vec<Encoding> {
        let mut list = vec![
            Encoding::Tight,
            Encoding::Zrle,
            Encoding::Hextile,
            Encoding::CopyRect,
            Encoding::Raw,
            Encoding::ExtendedDesktopSize,
            Encoding::DesktopSize,
            Encoding::LastRect,
            Encoding::CursorWithAlpha,
            Encoding::Cursor,
        ];
        list.push(Encoding::JpegQuality(self.config.quality.unwrap_or(8)));
        list.push(Encoding::CompressLevel(self.config.compress.unwrap_or(2)));
        list
    }
}
