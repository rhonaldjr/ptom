//! The agent-side connection state machine (sans-I/O).
//!
//! The server speaks first: [`ServerConnection::new`] queues the version
//! message. Client bytes are only ever interpreted according to the current
//! phase, so input messages sent before authentication completes are never
//! parsed as input (they are fed to the authenticator and fail there).

use crate::auth::{AuthProgress, Authenticator, TransportUpgrade};
use crate::buffer::InputBuffer;
use crate::client::incomplete_as_none;
use crate::connection::{
    ProtocolError, SECURITY_RESULT_FAILED, SECURITY_RESULT_OK, has_security_result,
};
use crate::handshake::{ServerInit, put_reason};
use crate::messages::{CutText, RectHeader};
use crate::wire::{Reader, WriteExt};
use crate::{ClientMessage, DecodeError, Limits, ProtocolVersion, SecurityType, ServerMessage};

/// Agent connection settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Newest protocol version to offer.
    pub max_version: ProtocolVersion,
    /// Security types to offer, most preferred first. The caller has already
    /// applied policy (e.g. legacy types only on loopback).
    pub security_types: Vec<SecurityType>,
    /// Bounds on client-announced lengths.
    pub limits: Limits,
}

/// Something the agent needs to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// The client chose this security type. Call
    /// [`ServerConnection::start_authentication`].
    SecuritySelected(SecurityType),
    /// Upgrade the transport, then call [`ServerConnection::transport_upgraded`].
    UpgradeTransport(TransportUpgrade),
    /// Authentication succeeded; the client sent `ClientInit`. Call
    /// [`ServerConnection::send_server_init`].
    ClientInit {
        /// Whether the client asked to share the desktop with other viewers.
        shared: bool,
    },
    /// A normal-phase message from the client.
    Message(ClientMessage),
    /// Clipboard text over the size limit was discarded.
    CutTextDiscarded {
        /// Announced length.
        len: u32,
    },
}

enum Phase {
    AwaitVersion,
    AwaitSecuritySelection,
    AwaitAuthenticator(SecurityType),
    Authenticating {
        security: SecurityType,
        auth: Box<dyn Authenticator>,
    },
    AwaitTransportUpgrade {
        security: SecurityType,
        auth: Box<dyn Authenticator>,
    },
    AwaitClientInit,
    AwaitServerInit,
    Normal,
    Skipping(usize),
    Closed,
}

/// The agent side of one RFB connection.
pub struct ServerConnection {
    config: ServerConfig,
    version: ProtocolVersion,
    phase: Phase,
    input: InputBuffer,
    output: Vec<u8>,
    /// The peer sent more than `buffer::MAX_UNREAD` without it being consumed.
    overflow: bool,
}

impl ServerConnection {
    /// Starts a connection and queues the server's version message.
    #[must_use]
    pub fn new(config: ServerConfig) -> Self {
        let version = config.max_version;
        Self {
            config,
            version,
            phase: Phase::AwaitVersion,
            input: InputBuffer::default(),
            output: version.to_bytes().to_vec(),
            overflow: false,
        }
    }

    /// The negotiated protocol version.
    #[must_use]
    pub fn version(&self) -> ProtocolVersion {
        self.version
    }

    /// Whether the handshake has completed.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self.phase, Phase::Normal | Phase::Skipping(_))
    }

    /// Appends bytes received from the client.
    pub fn receive(&mut self, bytes: &[u8]) {
        if !self.input.extend(bytes) {
            self.overflow = true;
        }
    }

    /// Takes the bytes that must be sent to the client. Call this after an
    /// error too: the output may carry the failure reason.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Supplies the authenticator for the security type the client chose.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no selection is pending or the type
    /// does not match.
    pub fn start_authentication(
        &mut self,
        security: SecurityType,
        auth: Box<dyn Authenticator>,
    ) -> Result<(), ProtocolError> {
        match self.phase {
            Phase::AwaitAuthenticator(selected) if selected == security => {
                self.phase = Phase::Authenticating { security, auth };
                Ok(())
            }
            _ => Err(ProtocolError::InvalidState(
                "no matching security selection pending",
            )),
        }
    }

    /// Reports that a requested transport upgrade is in place.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] if no upgrade was requested.
    pub fn transport_upgraded(&mut self) -> Result<(), ProtocolError> {
        match std::mem::replace(&mut self.phase, Phase::Closed) {
            Phase::AwaitTransportUpgrade { security, auth } => {
                // Bytes that arrived in the clear before the upgrade must not
                // be read as if they came through the protected channel
                // (STARTTLS-style injection, CWE-74).
                if !self.input.unread().is_empty() {
                    self.phase = Phase::Closed;
                    return Err(ProtocolError::Decode(DecodeError::Malformed(
                        "client sent unprotected data across a transport upgrade",
                    )));
                }
                self.phase = Phase::Authenticating { security, auth };
                Ok(())
            }
            other => {
                self.phase = other;
                Err(ProtocolError::InvalidState("no transport upgrade pending"))
            }
        }
    }

    /// Sends `ServerInit` and enters the normal phase.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] unless a `ClientInit` was just received.
    pub fn send_server_init(&mut self, init: &ServerInit) -> Result<(), ProtocolError> {
        if !matches!(self.phase, Phase::AwaitServerInit) {
            return Err(ProtocolError::InvalidState("ClientInit not received"));
        }
        init.encode(&mut self.output)?;
        self.phase = Phase::Normal;
        Ok(())
    }

    /// Queues a normal-phase message for the client.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] before the handshake completes.
    pub fn send(&mut self, message: &ServerMessage) -> Result<(), ProtocolError> {
        self.require_connected()?;
        message.encode(&mut self.output)?;
        Ok(())
    }

    /// Queues one rectangle of a framebuffer update: its header and the
    /// already-encoded payload.
    ///
    /// # Errors
    /// [`ProtocolError::InvalidState`] before the handshake completes.
    pub fn send_rect(&mut self, header: &RectHeader, payload: &[u8]) -> Result<(), ProtocolError> {
        self.require_connected()?;
        header.encode(&mut self.output);
        self.output.extend_from_slice(payload);
        Ok(())
    }

    /// Processes buffered input until an event is produced or more input is
    /// needed (`Ok(None)`).
    ///
    /// # Errors
    /// Any [`ProtocolError`]; the connection is then closed. Flush
    /// [`Self::take_output`] before dropping it.
    pub fn poll(&mut self) -> Result<Option<ServerEvent>, ProtocolError> {
        let result = if self.overflow {
            Err(overflow_error())
        } else {
            self.step()
        };
        if result.is_err() {
            self.phase = Phase::Closed;
        }
        result
    }

    fn require_connected(&self) -> Result<(), ProtocolError> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(ProtocolError::InvalidState("handshake not complete"))
        }
    }

    fn step(&mut self) -> Result<Option<ServerEvent>, ProtocolError> {
        loop {
            match self.phase {
                Phase::AwaitVersion => {
                    if let Some(event) = incomplete_as_none(self.read_version())? {
                        return Ok(Some(event));
                    }
                    if matches!(self.phase, Phase::AwaitVersion) {
                        return Ok(None);
                    }
                }
                Phase::AwaitSecuritySelection => return incomplete_as_none(self.read_selection()),
                Phase::Authenticating { .. } => match self.authenticate()? {
                    Step::Event(event) => return Ok(Some(event)),
                    Step::Progressed => {}
                    Step::Waiting => return Ok(None),
                },
                Phase::AwaitClientInit => return incomplete_as_none(self.read_client_init()),
                Phase::Normal => return incomplete_as_none(self.read_message()),
                Phase::Skipping(left) => {
                    let n = left.min(self.input.unread().len());
                    if n == 0 {
                        return Ok(None);
                    }
                    self.input.consume(n);
                    self.phase = if n == left {
                        Phase::Normal
                    } else {
                        Phase::Skipping(left - n)
                    };
                }
                Phase::AwaitAuthenticator(_)
                | Phase::AwaitTransportUpgrade { .. }
                | Phase::AwaitServerInit
                | Phase::Closed => return Ok(None),
            }
        }
    }

    fn read_version(&mut self) -> Result<Option<ServerEvent>, ProtocolError> {
        let requested = ProtocolVersion::decode(self.input.unread())?;
        self.input.consume(crate::version::VERSION_MESSAGE_LEN);
        self.version = ProtocolVersion::negotiate(requested, self.config.max_version);
        if self.version == ProtocolVersion::V3_3 {
            return self.dictate_security_type().map(Some);
        }
        let types = &self.config.security_types;
        if types.is_empty() {
            self.output.put_u8(0);
            put_reason(&mut self.output, "no security types available");
            return Err(ProtocolError::Refused(
                "no security types configured".into(),
            ));
        }
        self.output
            .put_u8(u8::try_from(types.len()).unwrap_or(u8::MAX));
        self.output.extend(types.iter().take(255).map(|t| t.code()));
        self.phase = Phase::AwaitSecuritySelection;
        Ok(None)
    }

    /// RFB 3.3: the server picks the type; only None and VncAuth exist there.
    fn dictate_security_type(&mut self) -> Result<ServerEvent, ProtocolError> {
        let chosen = self
            .config
            .security_types
            .iter()
            .copied()
            .find(|t| matches!(t, SecurityType::None | SecurityType::VncAuth));
        let Some(security) = chosen else {
            self.output.put_u32(0);
            put_reason(
                &mut self.output,
                "client too old: RFB 3.3 cannot use the required security",
            );
            return Err(ProtocolError::Refused(
                "RFB 3.3 client cannot meet security policy".into(),
            ));
        };
        self.output.put_u32(u32::from(security.code()));
        self.phase = Phase::AwaitAuthenticator(security);
        Ok(ServerEvent::SecuritySelected(security))
    }

    fn read_selection(&mut self) -> Result<Option<ServerEvent>, ProtocolError> {
        let code = Reader::new(self.input.unread()).u8()?;
        self.input.consume(1);
        let security = SecurityType::from_code(code);
        if !self.config.security_types.contains(&security) {
            self.fail_security("security type not offered");
            return Err(ProtocolError::Decode(DecodeError::Malformed(
                "client chose an unoffered security type",
            )));
        }
        self.phase = Phase::AwaitAuthenticator(security);
        Ok(Some(ServerEvent::SecuritySelected(security)))
    }

    fn authenticate(&mut self) -> Result<Step, ProtocolError> {
        let Phase::Authenticating { security, auth } = &mut self.phase else {
            return Ok(Step::Waiting);
        };
        let security = *security;
        let (used, progress) = match auth.advance(self.input.unread(), &mut self.output) {
            Ok(step) => step,
            Err(error) => {
                if has_security_result(self.version, security) {
                    self.fail_security(&error.reason);
                }
                return Err(error.into());
            }
        };
        self.input.consume(used);
        match progress {
            AuthProgress::NeedMore => Ok(if used > 0 {
                Step::Progressed
            } else {
                Step::Waiting
            }),
            AuthProgress::UpgradeTransport(upgrade) => {
                if let Phase::Authenticating { security, auth } =
                    std::mem::replace(&mut self.phase, Phase::Closed)
                {
                    self.phase = Phase::AwaitTransportUpgrade { security, auth };
                }
                Ok(Step::Event(ServerEvent::UpgradeTransport(upgrade)))
            }
            AuthProgress::Paused(_) => Err(ProtocolError::InvalidState(
                "server authenticators cannot pause",
            )),
            AuthProgress::Complete => {
                if has_security_result(self.version, security) {
                    self.output.put_u32(SECURITY_RESULT_OK);
                }
                self.phase = Phase::AwaitClientInit;
                Ok(Step::Progressed)
            }
        }
    }

    fn fail_security(&mut self, reason: &str) {
        self.output.put_u32(SECURITY_RESULT_FAILED);
        if self.version == ProtocolVersion::V3_8 {
            put_reason(&mut self.output, reason);
        }
    }

    fn read_client_init(&mut self) -> Result<Option<ServerEvent>, ProtocolError> {
        let shared = Reader::new(self.input.unread()).u8()? != 0;
        self.input.consume(1);
        self.phase = Phase::AwaitServerInit;
        Ok(Some(ServerEvent::ClientInit { shared }))
    }

    fn read_message(&mut self) -> Result<Option<ServerEvent>, ProtocolError> {
        let (message, used) = ClientMessage::decode(self.input.unread(), &self.config.limits)?;
        self.input.consume(used);
        Ok(Some(match message {
            ClientMessage::ClientCutText(CutText::Oversized { len }) => {
                self.phase = Phase::Skipping(len as usize);
                ServerEvent::CutTextDiscarded { len }
            }
            other => ServerEvent::Message(other),
        }))
    }
}

enum Step {
    Event(ServerEvent),
    Progressed,
    Waiting,
}

fn overflow_error() -> ProtocolError {
    ProtocolError::Decode(DecodeError::LimitExceeded {
        what: "unread input",
        len: crate::buffer::MAX_UNREAD as u64 + 1,
        max: crate::buffer::MAX_UNREAD as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, Authenticator};

    /// Asks for TLS right away (like VeNCrypt after its subtype ack).
    struct UpgradeNow;
    impl Authenticator for UpgradeNow {
        fn advance(
            &mut self,
            _input: &[u8],
            _output: &mut Vec<u8>,
        ) -> Result<(usize, AuthProgress), AuthError> {
            Ok((0, AuthProgress::UpgradeTransport(TransportUpgrade::Tls)))
        }
    }

    #[test]
    fn plaintext_left_over_at_a_transport_upgrade_is_rejected() {
        let mut server = ServerConnection::new(ServerConfig {
            max_version: ProtocolVersion::V3_8,
            security_types: vec![SecurityType::VeNCrypt],
            limits: Limits::DEFAULT,
        });
        server.receive(&ProtocolVersion::V3_8.to_bytes());
        server.receive(&[SecurityType::VeNCrypt.code()]);
        assert_eq!(
            server.poll(),
            Ok(Some(ServerEvent::SecuritySelected(SecurityType::VeNCrypt)))
        );
        server
            .start_authentication(SecurityType::VeNCrypt, Box::new(UpgradeNow))
            .unwrap();
        assert_eq!(
            server.poll(),
            Ok(Some(ServerEvent::UpgradeTransport(TransportUpgrade::Tls)))
        );
        // A man in the middle appends plaintext that must not be read as if it
        // came through TLS (STARTTLS command injection).
        server.receive(b"injected");
        assert!(matches!(
            server.transport_upgraded(),
            Err(ProtocolError::Decode(DecodeError::Malformed(_)))
        ));
    }

    #[test]
    fn a_peer_streaming_unread_data_is_cut_off() {
        let mut server = ServerConnection::new(ServerConfig {
            max_version: ProtocolVersion::V3_8,
            security_types: vec![SecurityType::None],
            limits: Limits::DEFAULT,
        });
        let chunk = vec![0u8; 1024 * 1024];
        for _ in 0..40 {
            server.receive(&chunk);
        }
        assert!(matches!(
            server.poll(),
            Err(ProtocolError::Decode(DecodeError::LimitExceeded { .. }))
        ));
    }
}
