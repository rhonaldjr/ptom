//! Viewer-side connection driver.

use std::net::SocketAddr;

use bytes::Bytes;

use farsight_client::{ServerAddress, Session, SessionConfig, SessionEvent, SessionStatus};
use farsight_rfb::SecurityType;
use farsight_rfb::auth::TransportUpgrade;
use secrecy::SecretString;
use thiserror::Error;
use tokio::net::TcpStream;

use crate::transport::Transport;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Input and control from the UI to a running connection.
#[derive(Debug)]
pub enum Command {
    /// Key press/release (X11 keysym).
    Key {
        /// Pressed or released.
        down: bool,
        /// X11 keysym.
        keysym: u32,
    },
    /// Key press/release with its physical position (Linux evdev code):
    /// sent as a QEMU extended key event when the server supports them.
    PhysicalKey {
        /// Pressed or released.
        down: bool,
        /// X11 keysym from the local layout.
        keysym: u32,
        /// Linux evdev code.
        evdev: u16,
    },
    /// Pointer state in framebuffer coordinates.
    Pointer {
        /// Button mask.
        buttons: u8,
        /// Horizontal position.
        x: u16,
        /// Vertical position.
        y: u16,
    },
    /// Local clipboard text.
    Clipboard(String),
    /// Answer to [`SessionEvent::CredentialsRequired`].
    Credentials {
        /// Username, when one was requested.
        username: Option<SecretString>,
        /// Password.
        password: SecretString,
    },
    /// Trust the identity shown with [`SessionEvent::IdentityConfirmationRequired`].
    ConfirmIdentity,
    /// Answer to [`SessionEvent::InsecureConsentRequired`].
    ConsentInsecure,
    /// Toggle view-only mode.
    SetViewOnly(bool),
    /// Ask the server to resize its desktop.
    Resize {
        /// Desired width.
        width: u16,
        /// Desired height.
        height: u16,
    },
    /// Close the connection.
    Disconnect,
}

/// A snapshot of the remote screen as tightly packed RGBA bytes.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA (cheap to clone).
    pub rgba: Bytes,
}

/// The remote cursor shape as straight-alpha RGBA.
#[derive(Debug, Clone)]
pub struct CursorImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Hot spot (the pixel that points).
    pub hotspot: (u16, u16),
    /// `width * height * 4` bytes, RGBA.
    pub rgba: Bytes,
}

/// Something that happened on the connection.
#[derive(Debug, Clone)]
pub enum Update {
    /// A session event (prompts, connected, bell, clipboard, …).
    Event(SessionEvent),
    /// New screen contents after a completed update.
    Frame(Frame),
    /// The remote cursor changed; `None` hides it.
    Cursor(Option<CursorImage>),
    /// The connection ended; `Some(reason)` if it failed.
    Closed(Option<String>),
}

/// Why a connection could not be opened.
#[derive(Debug, Error)]
pub enum ConnectError {
    /// The host name did not resolve.
    #[error("could not find {host}: {source}")]
    Resolve {
        /// Host that failed.
        host: String,
        /// Underlying error.
        source: std::io::Error,
    },
    /// No resolved address accepted a connection.
    #[error("could not connect to {0}: {1}.{hint}", hint = connect_hint(.1))]
    Connect(String, std::io::Error),
}

/// What a failed connection usually means, in plain words (user feedback:
/// "Connection refused" alone didn't say what to do).
fn connect_hint(error: &std::io::Error) -> &'static str {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => {
            " Nothing there is accepting remote-desktop connections. If it's a Farsight computer, \
             open Farsight Sharing on it, turn sharing on and choose \"Computers on my network\"."
        }
        ErrorKind::TimedOut | ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => {
            " The computer didn't answer: check that it's on and on the same network."
        }
        _ => "",
    }
}

/// Sends commands to a running connection.
#[derive(Debug, Clone)]
pub struct ConnectionHandle {
    commands: mpsc::UnboundedSender<Command>,
}

impl ConnectionHandle {
    /// Queues a command; returns false if the connection has ended.
    #[must_use]
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }
}

/// Connects to `address` and starts driving a session on the current tokio
/// runtime. Updates arrive on the returned receiver until `Update::Closed`.
///
/// # Errors
/// [`ConnectError`] if the host can't be resolved or reached.
pub async fn connect(
    address: &ServerAddress,
    mut config: SessionConfig,
) -> Result<(ConnectionHandle, mpsc::UnboundedReceiver<Update>), ConnectError> {
    let (stream, peer) = open(address).await?;
    config.loopback = peer.ip().is_loopback();
    let _ = stream.set_nodelay(true);
    let address_host = address.host.clone();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();
    let (updates_tx, updates_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let host = address_host;
        let reason = drive(
            Transport::new(stream),
            &host,
            Session::new(config),
            commands_rx,
            &updates_tx,
        )
        .await
        .err();
        let _ = updates_tx.send(Update::Closed(reason));
    });
    Ok((
        ConnectionHandle {
            commands: commands_tx,
        },
        updates_rx,
    ))
}

async fn open(address: &ServerAddress) -> Result<(TcpStream, SocketAddr), ConnectError> {
    let candidates = tokio::net::lookup_host((address.host.as_str(), address.port))
        .await
        .map_err(|source| ConnectError::Resolve {
            host: address.host.clone(),
            source,
        })?;
    let mut last_error = std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses");
    for candidate in candidates {
        match TcpStream::connect(candidate).await {
            Ok(stream) => return Ok((stream, candidate)),
            Err(e) => last_error = e,
        }
    }
    Err(ConnectError::Connect(address.display_name(), last_error))
}

async fn drive(
    mut transport: Transport,
    host: &str,
    mut session: Session,
    mut commands: mpsc::UnboundedReceiver<Command>,
    updates: &mpsc::UnboundedSender<Update>,
) -> Result<(), String> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let events = tokio::select! {
            read = transport.read(&mut buf) => {
                let Some(bytes) = read.map_err(|e| e.to_string())? else {
                    return if session.status() == SessionStatus::Connected {
                        Ok(())
                    } else {
                        Err("the server closed the connection during the handshake".into())
                    };
                };
                session.receive(&bytes);
                session.poll().map_err(|e| e.to_string())?
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()) };
                match apply(&mut session, command).map_err(|e| e.to_string())? {
                    Some(events) => events,
                    None => return Ok(()),
                }
            }
        };
        process(&mut transport, host, &mut session, events, updates).await?;
    }
}

/// Publishes events, sends pending output, and performs any transport
/// upgrade the session asked for (repeating until the session settles).
async fn process(
    transport: &mut Transport,
    host: &str,
    session: &mut Session,
    mut events: Vec<SessionEvent>,
    updates: &mpsc::UnboundedSender<Update>,
) -> Result<(), String> {
    loop {
        let upgrade = events.iter().find_map(|e| match e {
            SessionEvent::TransportUpgrade(u) => Some(u.clone()),
            _ => None,
        });
        events.retain(|e| !matches!(e, SessionEvent::TransportUpgrade(_)));
        publish(session, events, updates);
        // Output queued before an upgrade still goes out on the old transport.
        transport
            .write(&session.take_output())
            .await
            .map_err(|e| e.to_string())?;
        let Some(upgrade) = upgrade else {
            return Ok(());
        };
        events = match upgrade {
            TransportUpgrade::Tls => {
                let identity = transport
                    .upgrade_tls(host)
                    .await
                    .map_err(|e| format!("TLS failed: {e}"))?;
                let mut events: Vec<_> = session
                    .tls_established(identity.fingerprint, identity.valid_chain)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .collect();
                events.extend(session.poll().map_err(|e| e.to_string())?);
                events
            }
            TransportUpgrade::Ra2 {
                send_key,
                receive_key,
            } => {
                transport
                    .enable_ra2(&send_key, &receive_key)
                    .map_err(|e| e.to_string())?;
                session.transport_upgraded().map_err(|e| e.to_string())?;
                session.poll().map_err(|e| e.to_string())?
            }
            _ => return Err("unsupported transport upgrade".into()),
        };
    }
}

/// Applies a UI command, returning the resulting events, or `None` when the
/// UI asked to disconnect.
fn apply(
    session: &mut Session,
    command: Command,
) -> Result<Option<Vec<SessionEvent>>, farsight_rfb::ProtocolError> {
    match command {
        Command::Key { down, keysym } => session.key(down, keysym),
        Command::PhysicalKey {
            down,
            keysym,
            evdev,
        } => session.key_with_code(down, keysym, evdev),
        Command::Pointer { buttons, x, y } => session.pointer(buttons, x, y),
        Command::Clipboard(text) => session.clipboard(&text),
        Command::SetViewOnly(view_only) => session.set_view_only(view_only),
        Command::Resize { width, height } => {
            session.request_resize(width, height);
        }
        Command::Credentials { username, password } => {
            session.provide_credentials(username, password)?;
            return session.poll().map(Some);
        }
        Command::ConfirmIdentity => {
            session.confirm_identity()?;
            return session.poll().map(Some);
        }
        Command::ConsentInsecure => {
            warn!("user accepted an unencrypted connection");
            let mut events: Vec<_> = session.consent_insecure()?.into_iter().collect();
            events.extend(session.poll()?);
            return Ok(Some(events));
        }
        Command::Disconnect => return Ok(None),
    }
    Ok(Some(Vec::new()))
}

fn publish(session: &Session, events: Vec<SessionEvent>, updates: &mpsc::UnboundedSender<Update>) {
    let mut frame_due = false;
    for event in events {
        match &event {
            SessionEvent::UpdateComplete | SessionEvent::Resized { .. } => frame_due = true,
            SessionEvent::Connected {
                security,
                encrypted: false,
                ..
            } if *security != SecurityType::None => {
                warn!(?security, "connected without encryption");
            }
            SessionEvent::Damaged(_) => continue,
            SessionEvent::CursorChanged => {
                let cursor = session
                    .decoder()
                    .cursor()
                    .filter(|c| !c.is_empty())
                    .map(|c| CursorImage {
                        width: u32::from(c.width()),
                        height: u32::from(c.height()),
                        hotspot: c.hotspot(),
                        rgba: Bytes::copy_from_slice(c.rgba()),
                    });
                let _ = updates.send(Update::Cursor(cursor));
                continue;
            }
            _ => {}
        }
        debug!(?event, "session event");
        let _ = updates.send(Update::Event(event));
    }
    if frame_due {
        let _ = updates.send(Update::Frame(snapshot(session)));
    }
}

fn snapshot(session: &Session) -> Frame {
    let fb = session.framebuffer();
    let mut rgba = Vec::with_capacity(fb.pixels().len() * 4);
    for &p in fb.pixels() {
        let [_, r, g, b] = p.to_be_bytes();
        rgba.extend_from_slice(&[r, g, b, 0xff]);
    }
    Frame {
        width: u32::from(fb.width()),
        height: u32::from(fb.height()),
        rgba: Bytes::from(rgba),
    }
}
