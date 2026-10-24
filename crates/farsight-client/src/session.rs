//! One viewer connection: handshake policy, update loop, input and clipboard.

use farsight_codec::{Decoder, Framebuffer};
use farsight_rfb::auth::{Authenticator, NoAuth};
use farsight_rfb::client::{ClientConfig, ClientConnection, ClientEvent};
use farsight_rfb::{
    ClientMessage, CutText, Encoding, Limits, PixelFormat, ProtocolError, Rect, RectHeader, Screen,
    SecurityType,
};
use farsight_security::SecurityPolicy;
use farsight_security::vnc_auth::VncAuthClient;
use secrecy::SecretString;

/// Security types this build can complete, strongest first.
const IMPLEMENTED_SECURITY: [SecurityType; 2] = [SecurityType::VncAuth, SecurityType::None];

/// How a [`Session`] should connect.
#[allow(clippy::struct_excessive_bools)] // Independent user settings.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// The server address resolved to a loopback IP (legacy security allowed).
    pub loopback: bool,
    /// The user already agreed to an unencrypted connection to this server.
    pub allow_insecure: bool,
    /// Password, if already known (from the vault or a password file).
    pub password: Option<SecretString>,
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
    /// Paused until [`Session::provide_password`].
    WaitingForPassword,
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
    /// Ask the user for the password, then call [`Session::provide_password`].
    PasswordRequired,
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
    Password(SecurityType),
    InsecureConsent(SecurityType),
}

/// One viewer connection, independent of sockets and UI.
pub struct Session {
    config: SessionConfig,
    connection: ClientConnection,
    decoder: Decoder,
    status: SessionStatus,
    pending: Option<Pending>,
    security: Option<SecurityType>,
    last_clipboard: Option<String>,
}

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
            last_clipboard: None,
        }
    }

    /// Current lifecycle state.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        self.status
    }

    /// Whether traffic is encrypted (false for legacy security types).
    #[must_use]
    pub fn encrypted(&self) -> bool {
        false
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

    /// Supplies the password requested by [`SessionEvent::PasswordRequired`].
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no password was requested.
    pub fn provide_password(&mut self, password: SecretString) -> Result<(), ProtocolError> {
        let Some(Pending::Password(security)) = self.pending.take() else {
            return Err(ProtocolError::InvalidState("no password requested"));
        };
        self.config.password = Some(password);
        self.choose(security)
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
                ClientEvent::UpgradeTransport(_) => {
                    return Err(ProtocolError::InvalidState(
                        "transport upgrades arrive in M3",
                    ));
                }
                ClientEvent::Connected(init) => {
                    self.on_connected(init.width, init.height)?;
                    events.push(SessionEvent::Connected {
                        name: init.name,
                        width: init.width,
                        height: init.height,
                        security: self.security.unwrap_or(SecurityType::None),
                        encrypted: self.encrypted(),
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

    fn on_security_types(
        &mut self,
        offered: &[SecurityType],
    ) -> Result<Option<SessionEvent>, ProtocolError> {
        let usable: Vec<_> = offered
            .iter()
            .copied()
            .filter(|t| IMPLEMENTED_SECURITY.contains(t))
            .collect();
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
            self.status = SessionStatus::WaitingForPassword;
            return Ok(Some(SessionEvent::PasswordRequired));
        }
        self.choose(security)?;
        Ok(None)
    }

    fn choose(&mut self, security: SecurityType) -> Result<(), ProtocolError> {
        let auth: Box<dyn Authenticator> = match (security, &self.config.password) {
            (SecurityType::VncAuth, Some(password)) => {
                Box::new(VncAuthClient::new(password.clone()))
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
