//! Active RFB probing (ADR-0012). **Start only after the user's consent** for
//! the current network.
//!
//! Each probe connects, reads the server's `ProtocolVersion`, answers with
//! our version, reads the security-type list using the `farsight-rfb` client
//! state machine, and closes. It never sends a security-type choice,
//! authentication bytes or credentials: the only bytes written are the
//! 12-byte version message.

use std::net::{IpAddr, SocketAddr};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;

use farsight_rfb::client::{ClientConfig, ClientConnection, ClientEvent, RectDecoder};
use farsight_rfb::{DecodeError, ProtocolError, RectHeader};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::debug;

use crate::model::{DiscoveredServer, Provenance, RfbInfo, ServerKey, Verification};
use crate::scope::{ProbeScope, ScopeHost};
use crate::txt::TxtInfo;

/// Ports probed by default (VNC displays :0 to :10).
pub const DEFAULT_PORTS: RangeInclusive<u16> = 5900..=5910;
/// The most connections a probe keeps open at once.
pub const MAX_CONCURRENCY: usize = 32;
/// The most ports probed per host.
pub const MAX_PORTS: usize = 64;
/// Default time allowed for a TCP connect.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// Default time allowed for the whole read of version and security list.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_millis(1500);

/// Bytes read before a peer is declared not to be an RFB server. A version
/// (12) plus the longest security list (1 + 255) or failure reason fits well
/// within this.
const MAX_HANDSHAKE_BYTES: usize = 4096;

/// What to do with a host whose first port does not answer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SilentHostPolicy {
    /// Skip the host's remaining ports unless it is known to exist (it is in
    /// the neighbour table). Keeps a /24 scan within the 30 s budget when most
    /// addresses are unused, at the cost of missing hosts that silently drop
    /// the first port but accept a later one.
    #[default]
    SkipRemainingPorts,
    /// Try every port on every host.
    ProbeAllPorts,
}

/// Probe settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeConfig {
    /// Ports to probe on each host (deduplicated; at most [`MAX_PORTS`]).
    pub ports: Vec<u16>,
    /// Concurrent connections, clamped to `1..=`[`MAX_CONCURRENCY`].
    pub concurrency: usize,
    /// Time allowed for each TCP connect.
    pub connect_timeout: Duration,
    /// Time allowed, per connection, to receive the version and security list.
    pub read_timeout: Duration,
    /// Handling of hosts that don't answer.
    pub silent_hosts: SilentHostPolicy,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            ports: DEFAULT_PORTS.collect(),
            concurrency: MAX_CONCURRENCY,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
            silent_hosts: SilentHostPolicy::default(),
        }
    }
}

impl ProbeConfig {
    fn normalised(&self) -> Self {
        let mut ports: Vec<u16> = self.ports.iter().copied().filter(|p| *p != 0).collect();
        ports.sort_unstable();
        ports.dedup();
        ports.truncate(MAX_PORTS);
        Self {
            ports,
            concurrency: self.concurrency.clamp(1, MAX_CONCURRENCY),
            ..self.clone()
        }
    }
}

/// The result of probing one address and port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// An RFB server answered.
    Rfb(RfbInfo),
    /// The connection was refused or the host was unreachable.
    ConnectFailed,
    /// Nothing answered the connect in time.
    ConnectTimedOut,
    /// The connection opened but the version and security list didn't arrive in time.
    ReadTimedOut,
    /// The peer closed the connection before sending its security list.
    ClosedEarly,
    /// The peer sent something that is not an RFB handshake.
    NotRfb,
}

/// How far a probe has got, counted in hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeProgress {
    /// Hosts finished (every port tried or skipped).
    pub hosts_done: usize,
    /// Hosts in scope.
    pub hosts_total: usize,
}

/// Reported while [`probe`] runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeEvent {
    /// A server was found.
    Found(DiscoveredServer),
    /// Progress changed.
    Progress(ProbeProgress),
}

/// Probes every host in `scope` on the configured ports and returns the RFB
/// servers found. Finds and progress are also sent on `events` as they
/// happen; a closed receiver is ignored.
///
/// Cancel by dropping the returned future: in-flight connections are aborted.
pub async fn probe(
    scope: &ProbeScope,
    config: &ProbeConfig,
    events: mpsc::UnboundedSender<ProbeEvent>,
) -> Vec<DiscoveredServer> {
    let config = Arc::new(config.normalised());
    let permits = Arc::new(Semaphore::new(config.concurrency));
    let hosts_total = scope.len();
    let mut tasks = JoinSet::new();
    for host in scope.hosts().iter().copied() {
        tasks.spawn(probe_host(host, Arc::clone(&config), Arc::clone(&permits)));
    }
    let _ = events.send(ProbeEvent::Progress(ProbeProgress {
        hosts_done: 0,
        hosts_total,
    }));
    let mut found = Vec::new();
    let mut hosts_done = 0;
    while let Some(joined) = tasks.join_next().await {
        hosts_done += 1;
        for server in joined.unwrap_or_default() {
            let _ = events.send(ProbeEvent::Found(server.clone()));
            found.push(server);
        }
        let _ = events.send(ProbeEvent::Progress(ProbeProgress {
            hosts_done,
            hosts_total,
        }));
    }
    debug!(hosts = hosts_total, found = found.len(), "probe finished");
    found
}

async fn probe_host(
    host: ScopeHost,
    config: Arc<ProbeConfig>,
    permits: Arc<Semaphore>,
) -> Vec<DiscoveredServer> {
    let mut found = Vec::new();
    let mut answered = host.known_alive;
    for &port in &config.ports {
        let Ok(permit) = permits.acquire().await else {
            break;
        };
        let outcome = probe_one(SocketAddr::new(host.addr, port), &config).await;
        drop(permit);
        match outcome {
            ProbeOutcome::Rfb(info) => found.push(probed_server(host.addr, port, info)),
            ProbeOutcome::ConnectTimedOut
                if !answered && config.silent_hosts == SilentHostPolicy::SkipRemainingPorts =>
            {
                break;
            }
            ProbeOutcome::ConnectTimedOut => {}
            _ => answered = true,
        }
    }
    found
}

fn probed_server(addr: IpAddr, port: u16, rfb: RfbInfo) -> DiscoveredServer {
    DiscoveredServer {
        key: ServerKey::new(format!("probe:{}", SocketAddr::new(addr, port))),
        name: addr.to_string(),
        host_name: None,
        addresses: vec![addr],
        port,
        provenance: Provenance::Probe,
        verification: Verification::Unverified,
        txt: TxtInfo::default(),
        rfb: Some(rfb),
    }
}

/// Probes one address and port: connect, read the version, send ours, read
/// the security-type list, close.
pub async fn probe_one(target: SocketAddr, config: &ProbeConfig) -> ProbeOutcome {
    let stream = match timeout(config.connect_timeout, TcpStream::connect(target)).await {
        Err(_) => return ProbeOutcome::ConnectTimedOut,
        Ok(Err(_)) => return ProbeOutcome::ConnectFailed,
        Ok(Ok(stream)) => stream,
    };
    timeout(config.read_timeout, read_security_types(stream))
        .await
        .unwrap_or(ProbeOutcome::ReadTimedOut)
}

/// Reads the handshake through [`ProbeHandshake`]. The stream is dropped
/// (closed) on return.
async fn read_security_types(mut stream: TcpStream) -> ProbeOutcome {
    let mut handshake = ProbeHandshake::new();
    let mut buffer = [0u8; 512];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return ProbeOutcome::ClosedEarly,
            Ok(n) => n,
        };
        match handshake.feed(buffer.get(..read).unwrap_or_default()) {
            HandshakeStep::Finished(outcome) => return outcome,
            HandshakeStep::Continue { reply } => {
                if !reply.is_empty() && stream.write_all(&reply).await.is_err() {
                    return ProbeOutcome::ClosedEarly;
                }
            }
        }
    }
}

/// What to do after feeding bytes to a [`ProbeHandshake`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeStep {
    /// Send `reply` (possibly empty) and keep reading.
    Continue {
        /// Bytes for the server: only ever our 12-byte version message.
        reply: Vec<u8>,
    },
    /// Stop and close the connection. Nothing more may be sent.
    Finished(ProbeOutcome),
}

/// The sans-I/O part of a probe: wraps the `farsight-rfb` client state
/// machine and stops at the security-type list, so the only bytes it ever
/// asks to send are the `ProtocolVersion` reply.
pub struct ProbeHandshake {
    connection: ClientConnection,
    received: usize,
    finished: bool,
}

impl Default for ProbeHandshake {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ProbeHandshake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeHandshake")
            .field("received", &self.received)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl ProbeHandshake {
    /// Starts a handshake that waits for the server's version.
    #[must_use]
    pub fn new() -> Self {
        Self {
            connection: ClientConnection::new(ClientConfig::default()),
            received: 0,
            finished: false,
        }
    }

    /// Feeds bytes received from the server. Once it has returned
    /// [`HandshakeStep::Finished`], every further call returns
    /// `Finished(NotRfb)` and never a reply.
    pub fn feed(&mut self, bytes: &[u8]) -> HandshakeStep {
        if self.finished {
            return HandshakeStep::Finished(ProbeOutcome::NotRfb);
        }
        let step = self.advance(bytes);
        self.finished = matches!(step, HandshakeStep::Finished(_));
        step
    }

    fn advance(&mut self, bytes: &[u8]) -> HandshakeStep {
        self.received = self.received.saturating_add(bytes.len());
        if self.received > MAX_HANDSHAKE_BYTES {
            return HandshakeStep::Finished(ProbeOutcome::NotRfb);
        }
        self.connection.receive(bytes);
        let outcome = match self.connection.poll(&mut NoRectangles) {
            Ok(None) => {
                return HandshakeStep::Continue {
                    reply: self.connection.take_output(),
                };
            }
            Ok(Some(ClientEvent::SecurityTypes(types))) => {
                ProbeOutcome::Rfb(RfbInfo::new(self.connection.version(), types))
            }
            // An RFB server that is refusing connections (the reason is peer
            // text and is deliberately not kept).
            Err(ProtocolError::Refused(_)) => {
                ProbeOutcome::Rfb(RfbInfo::new(self.connection.version(), Vec::new()))
            }
            // No other event can precede the security list.
            Ok(Some(_)) | Err(_) => ProbeOutcome::NotRfb,
        };
        HandshakeStep::Finished(outcome)
    }
}

/// The probe stops before the normal phase, so it never decodes rectangles.
struct NoRectangles;

impl RectDecoder for NoRectangles {
    fn decode_rect(
        &mut self,
        _header: &RectHeader,
        _input: &[u8],
    ) -> Result<Option<usize>, DecodeError> {
        Err(DecodeError::Malformed("probe does not decode rectangles"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use farsight_rfb::{ProtocolVersion, SecurityType};
    use proptest::prelude::*;

    #[test]
    fn config_is_normalised() {
        let config = ProbeConfig {
            ports: vec![5901, 0, 5900, 5901],
            concurrency: 1000,
            ..ProbeConfig::default()
        }
        .normalised();
        assert_eq!(config.ports, vec![5900, 5901]);
        assert_eq!(config.concurrency, MAX_CONCURRENCY);
        let zero = ProbeConfig {
            concurrency: 0,
            ports: (1..=1000).collect(),
            ..ProbeConfig::default()
        }
        .normalised();
        assert_eq!(zero.concurrency, 1);
        assert_eq!(zero.ports.len(), MAX_PORTS);
    }

    /// Feeds `chunks` and returns every reply byte and the outcome.
    fn run(chunks: &[&[u8]]) -> (Vec<u8>, Option<ProbeOutcome>) {
        let mut handshake = ProbeHandshake::new();
        let mut sent = Vec::new();
        let mut outcome = None;
        for chunk in chunks {
            match handshake.feed(chunk) {
                HandshakeStep::Continue { reply } => sent.extend(reply),
                HandshakeStep::Finished(done) => {
                    outcome.get_or_insert(done);
                }
            }
        }
        (sent, outcome)
    }

    #[test]
    fn handshake_byte_by_byte() {
        let server: Vec<u8> = b"RFB 003.008\n"
            .iter()
            .chain(&[2, 19, 2])
            .copied()
            .collect();
        let chunks: Vec<&[u8]> = server.chunks(1).collect();
        let (sent, outcome) = run(&chunks);
        assert_eq!(sent, b"RFB 003.008\n");
        assert_eq!(
            outcome,
            Some(ProbeOutcome::Rfb(RfbInfo::new(
                ProtocolVersion::V3_8,
                vec![SecurityType::VeNCrypt, SecurityType::VncAuth]
            )))
        );
    }

    #[test]
    fn handshake_sends_nothing_after_the_list() {
        let (sent, outcome) = run(&[b"RFB 003.008\n", &[1, 2], b"more bytes from the server"]);
        assert_eq!(sent, b"RFB 003.008\n");
        assert!(matches!(outcome, Some(ProbeOutcome::Rfb(_))));
    }

    #[test]
    fn handshake_with_apple_and_3_3_versions() {
        let (sent, outcome) = run(&[b"RFB 003.889\n", &[1, 30]]);
        assert_eq!(sent, b"RFB 003.008\n");
        assert!(matches!(outcome, Some(ProbeOutcome::Rfb(_))));
        let (sent, outcome) = run(&[b"RFB 003.003\n", &[0, 0, 0, 2]]);
        assert_eq!(sent, b"RFB 003.003\n");
        assert_eq!(
            outcome,
            Some(ProbeOutcome::Rfb(RfbInfo::new(
                ProtocolVersion::V3_3,
                vec![SecurityType::VncAuth]
            )))
        );
    }

    #[test]
    fn handshake_rejects_garbage_and_floods() {
        assert_eq!(run(&[b"SSH-2.0-OpenSSH\r\n"]).1, Some(ProbeOutcome::NotRfb));
        let flood = vec![b'R'; MAX_HANDSHAKE_BYTES + 1];
        assert_eq!(
            run(&[b"RFB 003.008\n", &flood]).1,
            Some(ProbeOutcome::NotRfb)
        );
    }

    proptest! {
        #[test]
        fn handshake_only_ever_sends_one_version(
            chunks in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..64), 0..16),
            prefix in prop::sample::select(vec![
                &b""[..], b"RFB 003.008\n", b"RFB 003.003\n", b"RFB 003.007\n"]),
        ) {
            let mut all: Vec<&[u8]> = vec![prefix];
            all.extend(chunks.iter().map(Vec::as_slice));
            let (sent, _) = run(&all);
            prop_assert!(
                sent.is_empty()
                    || [&b"RFB 003.003\n"[..], b"RFB 003.007\n", b"RFB 003.008\n"].contains(&sent.as_slice()),
                "unexpected bytes sent: {:?}", sent
            );
        }
    }

    #[test]
    fn defaults_match_adr_0012() {
        let config = ProbeConfig::default();
        assert_eq!(config.ports, (5900..=5910).collect::<Vec<_>>());
        assert_eq!(config.concurrency, 32);
        assert_eq!(config.connect_timeout, Duration::from_millis(500));
        assert_eq!(config.read_timeout, Duration::from_millis(1500));
    }
}
