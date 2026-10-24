//! The viewer-side connection state machine (sans-I/O).
//!
//! Feed received bytes with [`ClientConnection::receive`], call
//! [`ClientConnection::poll`] until it returns `Ok(None)`, and send whatever
//! [`ClientConnection::take_output`] returns.

use crate::auth::{AuthProgress, Authenticator, TransportUpgrade};
use crate::buffer::InputBuffer;
use crate::connection::{ProtocolError, SECURITY_RESULT_OK, has_security_result};
use crate::handshake::{ServerInit, read_reason};
use crate::messages::{CutText, RECT_HEADER_LEN, RECTS_UNTIL_LAST_RECT, RectHeader};
use crate::wire::{Reader, WriteExt};
use crate::{
    ClientMessage, DecodeError, Encoding, Limits, ProtocolVersion, SecurityType, ServerMessage,
};

/// Decodes rectangle payloads into the viewer's framebuffer.
///
/// Implemented by `farsight-codec`. `decode_rect` must not apply partial
/// results: it returns `Ok(None)` (with no side effects) until the whole
/// payload is available.
pub trait RectDecoder {
    /// Decodes the payload that starts at `input`, returning the number of
    /// bytes it occupied, or `None` if more input is needed.
    ///
    /// # Errors
    /// [`DecodeError`] if the payload is invalid or the rectangle lies outside
    /// the framebuffer.
    fn decode_rect(
        &mut self,
        header: &RectHeader,
        input: &[u8],
    ) -> Result<Option<usize>, DecodeError>;
}

/// Viewer connection settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientConfig {
    /// Newest protocol version to speak.
    pub max_version: ProtocolVersion,
    /// Ask the server to keep other viewers connected.
    pub shared: bool,
    /// Bounds on server-announced lengths.
    pub limits: Limits,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_version: ProtocolVersion::LATEST,
            shared: true,
            limits: Limits::DEFAULT,
        }
    }
}

/// Something the viewer needs to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    /// The server offers these security types. Call
    /// [`ClientConnection::choose_security`].
    SecurityTypes(Vec<SecurityType>),
    /// Upgrade the transport, then call [`ClientConnection::transport_upgraded`].
    UpgradeTransport(TransportUpgrade),
    /// The handshake finished; normal messages may now be sent.
    Connected(ServerInit),
    /// A rectangle was decoded and applied to the framebuffer.
    Rect(RectHeader),
    /// The current framebuffer update is complete.
    UpdateComplete,
    /// New colour-map entries (16-bit RGB) starting at `first`.
    ColourMap {
        /// First index.
        first: u16,
        /// RGB values.
        colours: Vec<[u16; 3]>,
    },
    /// Ring the bell.
    Bell,
    /// Clipboard text from the server (Latin-1).
    ServerCutText(Vec<u8>),
    /// Clipboard text over the size limit was discarded.
    CutTextDiscarded {
        /// Announced length.
        len: u32,
    },
}

enum Phase {
    AwaitVersion,
    AwaitSecurityTypes,
    ChooseSecurity(Vec<SecurityType>),
    Authenticating {
        security: SecurityType,
        auth: Box<dyn Authenticator>,
    },
    AwaitTransportUpgrade {
        security: SecurityType,
        auth: Box<dyn Authenticator>,
    },
    AwaitSecurityResult,
    AwaitServerInit,
    Normal(Normal),
    Closed,
}

enum Normal {
    Idle,
    Update {
        remaining: Option<u16>,
        current: Option<RectHeader>,
    },
    Skipping(usize),
}

/// The viewer side of one RFB connection.
pub struct ClientConnection {
    config: ClientConfig,
    version: ProtocolVersion,
    phase: Phase,
    input: InputBuffer,
    output: Vec<u8>,
}

impl ClientConnection {
    /// Starts a connection that waits for the server's version message.
    #[must_use]
    pub fn new(config: ClientConfig) -> Self {
        Self {
            config,
            version: config.max_version,
            phase: Phase::AwaitVersion,
            input: InputBuffer::default(),
            output: Vec::new(),
        }
    }

    /// The negotiated protocol version (valid once the handshake has started).
    #[must_use]
    pub fn version(&self) -> ProtocolVersion {
        self.version
    }

    /// Whether the handshake has completed.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Normal(_))
    }

    /// Appends bytes received from the server.
    pub fn receive(&mut self, bytes: &[u8]) {
        self.input.extend(bytes);
    }

    /// Takes the bytes that must be sent to the server.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Chooses one of the offered security types and its authenticator.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no choice is pending or the type was
    /// not offered.
    pub fn choose_security(
        &mut self,
        security: SecurityType,
        auth: Box<dyn Authenticator>,
    ) -> Result<(), ProtocolError> {
        let Phase::ChooseSecurity(offered) = &self.phase else {
            return Err(ProtocolError::InvalidState("no security choice pending"));
        };
        if !offered.contains(&security) {
            return Err(ProtocolError::InvalidState("security type was not offered"));
        }
        if self.version != ProtocolVersion::V3_3 {
            self.output.put_u8(security.code());
        }
        self.phase = Phase::Authenticating { security, auth };
        Ok(())
    }

    /// Reports that a requested transport upgrade is in place.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no upgrade was requested.
    pub fn transport_upgraded(&mut self) -> Result<(), ProtocolError> {
        match std::mem::replace(&mut self.phase, Phase::Closed) {
            Phase::AwaitTransportUpgrade { security, auth } => {
                self.phase = Phase::Authenticating { security, auth };
                Ok(())
            }
            other => {
                self.phase = other;
                Err(ProtocolError::InvalidState("no transport upgrade pending"))
            }
        }
    }

    /// Queues a normal-phase message for the server.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] before the handshake completes.
    pub fn send(&mut self, message: &ClientMessage) -> Result<(), ProtocolError> {
        if !self.is_connected() {
            return Err(ProtocolError::InvalidState("handshake not complete"));
        }
        message.encode(&mut self.output)?;
        Ok(())
    }

    /// Processes buffered input until an event is produced or more input is
    /// needed (`Ok(None)`).
    ///
    /// # Errors
    /// Any [`ProtocolError`]; the connection is then closed.
    pub fn poll(
        &mut self,
        decoder: &mut dyn RectDecoder,
    ) -> Result<Option<ClientEvent>, ProtocolError> {
        let result = self.step(decoder);
        if result.is_err() {
            self.phase = Phase::Closed;
        }
        result
    }

    fn step(
        &mut self,
        decoder: &mut dyn RectDecoder,
    ) -> Result<Option<ClientEvent>, ProtocolError> {
        loop {
            let progressed = match self.phase {
                Phase::AwaitVersion => self.read_version()?,
                Phase::AwaitSecurityTypes => return incomplete_as_none(self.read_security_types()),
                Phase::ChooseSecurity(_) | Phase::AwaitTransportUpgrade { .. } | Phase::Closed => {
                    return Ok(None);
                }
                Phase::Authenticating { .. } => match self.authenticate()? {
                    Step::Event(event) => return Ok(Some(event)),
                    Step::Progressed => true,
                    Step::Waiting => false,
                },
                Phase::AwaitSecurityResult => self.read_security_result()?,
                Phase::AwaitServerInit => return incomplete_as_none(self.read_server_init()),
                Phase::Normal(_) => return self.read_normal(decoder),
            };
            if !progressed {
                return Ok(None);
            }
        }
    }

    fn read_version(&mut self) -> Result<bool, ProtocolError> {
        let offered = match ProtocolVersion::decode(self.input.unread()) {
            Err(e) if e.is_incomplete() => return Ok(false),
            other => other?,
        };
        self.input.consume(crate::version::VERSION_MESSAGE_LEN);
        self.version = ProtocolVersion::negotiate(offered, self.config.max_version);
        self.output.extend_from_slice(&self.version.to_bytes());
        self.phase = Phase::AwaitSecurityTypes;
        Ok(true)
    }

    fn read_security_types(&mut self) -> Result<Option<ClientEvent>, ProtocolError> {
        let mut r = Reader::new(self.input.unread());
        let offered = if self.version == ProtocolVersion::V3_3 {
            match r.u32()? {
                0 => {
                    return Err(ProtocolError::Refused(read_reason(
                        &mut r,
                        &self.config.limits,
                    )?));
                }
                code => vec![SecurityType::from_code(
                    u8::try_from(code)
                        .map_err(|_| DecodeError::Malformed("security type out of range"))?,
                )],
            }
        } else {
            match r.u8()? {
                0 => {
                    return Err(ProtocolError::Refused(read_reason(
                        &mut r,
                        &self.config.limits,
                    )?));
                }
                count => r
                    .bytes(usize::from(count))?
                    .iter()
                    .copied()
                    .map(SecurityType::from_code)
                    .collect(),
            }
        };
        let used = r.consumed();
        self.input.consume(used);
        self.phase = Phase::ChooseSecurity(offered.clone());
        Ok(Some(ClientEvent::SecurityTypes(offered)))
    }

    fn authenticate(&mut self) -> Result<Step, ProtocolError> {
        let Phase::Authenticating { security, auth } = &mut self.phase else {
            return Ok(Step::Waiting);
        };
        let (used, progress) = auth.advance(self.input.unread(), &mut self.output)?;
        self.input.consume(used);
        match progress {
            AuthProgress::NeedMore => Ok(if used > 0 {
                Step::Progressed
            } else {
                Step::Waiting
            }),
            AuthProgress::UpgradeTransport(upgrade) => {
                let Phase::Authenticating { security, auth } =
                    std::mem::replace(&mut self.phase, Phase::Closed)
                else {
                    return Ok(Step::Waiting);
                };
                self.phase = Phase::AwaitTransportUpgrade { security, auth };
                Ok(Step::Event(ClientEvent::UpgradeTransport(upgrade)))
            }
            AuthProgress::Complete => {
                if has_security_result(self.version, *security) {
                    self.phase = Phase::AwaitSecurityResult;
                } else {
                    self.send_client_init();
                }
                Ok(Step::Progressed)
            }
        }
    }

    fn read_security_result(&mut self) -> Result<bool, ProtocolError> {
        let mut r = Reader::new(self.input.unread());
        let status = match r.u32() {
            Err(e) if e.is_incomplete() => return Ok(false),
            other => other?,
        };
        if status == SECURITY_RESULT_OK {
            self.input.consume(r.consumed());
            self.send_client_init();
            return Ok(true);
        }
        let reason = if self.version == ProtocolVersion::V3_8 {
            match read_reason(&mut r, &self.config.limits) {
                Err(e) if e.is_incomplete() => return Ok(false),
                other => other?,
            }
        } else {
            "authentication failed".to_owned()
        };
        Err(ProtocolError::Refused(reason))
    }

    fn send_client_init(&mut self) {
        self.output.put_u8(u8::from(self.config.shared));
        self.phase = Phase::AwaitServerInit;
    }

    fn read_server_init(&mut self) -> Result<Option<ClientEvent>, ProtocolError> {
        let (init, used) = ServerInit::decode(self.input.unread(), &self.config.limits)?;
        self.input.consume(used);
        self.phase = Phase::Normal(Normal::Idle);
        Ok(Some(ClientEvent::Connected(init)))
    }

    fn read_normal(
        &mut self,
        decoder: &mut dyn RectDecoder,
    ) -> Result<Option<ClientEvent>, ProtocolError> {
        loop {
            let Phase::Normal(state) = &mut self.phase else {
                return Ok(None);
            };
            match state {
                Normal::Skipping(left) => {
                    let n = (*left).min(self.input.unread().len());
                    if n == 0 {
                        return Ok(None);
                    }
                    *left -= n;
                    if *left == 0 {
                        *state = Normal::Idle;
                    }
                    self.input.consume(n);
                }
                Normal::Idle => match incomplete_as_none(self.read_message())? {
                    Some(Message::Event(event)) => return Ok(Some(event)),
                    Some(Message::UpdateStarted) => {}
                    None => return Ok(None),
                },
                Normal::Update { remaining, current } => {
                    if *remaining == Some(0) {
                        *state = Normal::Idle;
                        return Ok(Some(ClientEvent::UpdateComplete));
                    }
                    match current {
                        None => {
                            let header = match RectHeader::decode(self.input.unread()) {
                                Err(e) if e.is_incomplete() => return Ok(None),
                                other => other?,
                            };
                            self.input.consume(RECT_HEADER_LEN);
                            if header.encoding == Encoding::LastRect {
                                *state = Normal::Idle;
                                return Ok(Some(ClientEvent::UpdateComplete));
                            }
                            *current = Some(header);
                        }
                        Some(header) => {
                            let header = *header;
                            let Some(used) = decoder.decode_rect(&header, self.input.unread())?
                            else {
                                return Ok(None);
                            };
                            *current = None;
                            if let Some(count) = remaining {
                                *count -= 1;
                            }
                            self.input.consume(used);
                            return Ok(Some(ClientEvent::Rect(header)));
                        }
                    }
                }
            }
        }
    }

    fn read_message(&mut self) -> Result<Option<Message>, ProtocolError> {
        let (message, used) = ServerMessage::decode(self.input.unread(), &self.config.limits)?;
        self.input.consume(used);
        let event = match message {
            ServerMessage::FramebufferUpdate { rects } => {
                let remaining = (rects != RECTS_UNTIL_LAST_RECT).then_some(rects);
                self.phase = Phase::Normal(Normal::Update {
                    remaining,
                    current: None,
                });
                return Ok(Some(Message::UpdateStarted));
            }
            ServerMessage::SetColourMapEntries { first, colours } => {
                ClientEvent::ColourMap { first, colours }
            }
            ServerMessage::Bell => ClientEvent::Bell,
            ServerMessage::ServerCutText(CutText::Text(text)) => ClientEvent::ServerCutText(text),
            ServerMessage::ServerCutText(CutText::Oversized { len }) => {
                self.phase = Phase::Normal(Normal::Skipping(len as usize));
                ClientEvent::CutTextDiscarded { len }
            }
        };
        Ok(Some(Message::Event(event)))
    }
}

enum Step {
    Event(ClientEvent),
    Progressed,
    Waiting,
}

enum Message {
    Event(ClientEvent),
    UpdateStarted,
}

/// Turns "need more input" into `Ok(None)`, keeping real errors.
pub(crate) fn incomplete_as_none<T>(
    result: Result<Option<T>, ProtocolError>,
) -> Result<Option<T>, ProtocolError> {
    match result {
        Err(ProtocolError::Decode(e)) if e.is_incomplete() => Ok(None),
        other => other,
    }
}
