//! `farsight-agent`: shares this desktop with Farsight and other VNC viewers.
//! Composition root (ADR-0018); the protocol lives in `farsight-server`.

mod advertise;
mod capture;
mod compat;
mod identity;
mod options;
mod runtime;
mod vncserver;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use farsight_server::UserVerifier;
use farsight_vault::KeyringKeySource;
use farsight_vault::agent::AgentCredentials;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::identity::{Identity, format_fingerprint};
use crate::options::{AgentOptions, Command, Listen, USAGE};
use crate::runtime::{Agent, CredentialsVerifier, Settings};

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    // Before anything else, for every personality (ADR-0009): as root, even
    // `vncserver` would write files in a user-controlled directory.
    if rustix::process::geteuid().is_root() {
        eprintln!("farsight-agent must not run as root; run it as the desktop user (ADR-0009)");
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().collect();
    let program = args.first().cloned().unwrap_or_default();
    let rest = args.get(1..).unwrap_or_default();
    let options = match compat::Personality::from_program(&program) {
        compat::Personality::Farsight => options::parse_farsight(settings_file_defaults, rest),
        compat::Personality::Vncserver => {
            let defaults = compat::vncserver_config(&vncserver::config_file());
            let mandatory = compat::mandatory_listen(&vncserver::mandatory_config_file());
            let command = compat::vncserver_options(rest, defaults).map(|command| {
                match (command, mandatory) {
                    // The administrator's `localhost` wins (TigerVNC's mandatory config).
                    (
                        compat::VncserverCommand::Start {
                            display, geometry, ..
                        },
                        Some(listen),
                    ) => compat::VncserverCommand::Start {
                        display,
                        geometry,
                        listen,
                    },
                    (command, _) => command,
                }
            });
            return match command.map(vncserver::run) {
                Ok(Ok(())) => ExitCode::SUCCESS,
                Ok(Err(error)) => {
                    eprintln!("vncserver: {error:#}");
                    ExitCode::FAILURE
                }
                Err(error) => {
                    eprintln!("vncserver: {error}\n{}", vncserver::USAGE);
                    ExitCode::from(2)
                }
            };
        }
        personality => match compat::scraping_options(personality, rest) {
            Ok(compat::ScrapingCommand::Run(mut options, display)) => {
                options.x11_display = display;
                Ok(Command::Run(options))
            }
            Ok(compat::ScrapingCommand::Help) => Ok(Command::Help),
            Err(error) => Err(error),
        },
    };
    let options = match options {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("farsight-agent {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("farsight-agent: {error}");
            return ExitCode::from(2);
        }
    };
    // Read (and clear) systemd's socket variables while still single-threaded.
    let passed = listenfd::ListenFd::from_env();
    // A headless desktop creates sockets (Xwayland's among them) that only
    // this user may use; the session's apps get the original umask back.
    let session_umask = (options.backend == options::Backend::Headless)
        .then(|| rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o077)).as_raw_mode());
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("farsight-agent: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(options, passed, session_umask)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("farsight-agent: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(
    options: AgentOptions,
    passed: listenfd::ListenFd,
    session_umask: Option<u32>,
) -> Result<()> {
    if rustix::process::geteuid().is_root() {
        bail!("farsight-agent must not run as root; run it as the desktop user (ADR-0009)");
    }
    // Catch SIGHUP from the start: a `systemctl --user reload` while we are
    // still starting must not kill us (its default action).
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    if let Some(trust) = options.change_trust {
        return advertise::change_trust(&Identity::default_dir()?, trust);
    }
    let host = host_name();
    let identity = Identity::load_or_create(&Identity::default_dir()?, &host)?;
    if options.show_identity {
        return show_identity(&identity);
    }

    let logins = load_logins_blocking(KeyringFailure::Continue).await?;
    // A socket handed over by systemd (socket activation) decides where we listen.
    let (listener, listen) = open_listener(passed, options.listen, options.port).await?;
    let port = listener.local_addr()?.port();
    let has_users = logins.verifier.is_some();
    let vnc_password = logins.vnc_password.clone();
    // A headless desktop has nobody at the screen to approve logins, so it
    // always needs a password, even on loopback (like vncserver).
    if options.backend == options::Backend::Headless && !has_users && vnc_password.is_none() {
        bail!(
            "a headless desktop needs a password, because nobody is at its screen to approve logins.\n\
             Run `farsight-passwd user add {}` first.",
            owner().unwrap_or_else(|| "<your-user>".into())
        );
    }
    if listen == Listen::Lan && !has_users && vnc_password.is_none() {
        bail!(
            "no passwords are set up, so the agent won't accept connections from the network.\n\
             Run `farsight-passwd user add {}` first.",
            owner().unwrap_or_else(|| "<your-user>".into())
        );
    }

    let backend = options.backend.resolve(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    );
    let token_file = portal_token_file();
    if options.forget_portal_approval {
        match std::fs::remove_file(&token_file) {
            Ok(()) => println!("Forgot the remembered screen-sharing approval."),
            Err(_) => println!("No screen-sharing approval was remembered."),
        }
        return Ok(());
    }
    let headless = backend == options::Backend::Headless;
    let mut setup = headless_setup(&options, headless, port)?;
    setup.session_umask = session_umask;
    let capture = capture::start(backend, &token_file, setup).await?;
    let settings = Settings {
        owner: owner(),
        tls: farsight_net::tls_acceptor(&identity.cert_pem, &identity.key_pem)?,
        ra2: Arc::clone(&identity.ra2),
        logins,
        allow_legacy: options.allow_legacy,
        view_only: options.view_only,
        headless,
    };
    let name = format!("{}@{host}", owner().unwrap_or_else(|| "desktop".into()));
    let agent = Agent::new(settings, name, capture.input.clone());
    agent.start_presence().await;

    info!(address = %listener.local_addr()?, ?backend, "sharing this desktop");
    // ADR-0010 + P-32: announce only when the network may connect, only on
    // networks the user marked as theirs, with a minimal TXT record.
    if listen == Listen::Lan && options.advertise {
        let config = farsight_discovery::AdvertiseConfig {
            instance_name: host.clone(),
            host_name: host.clone(),
            port,
            fingerprint: hex(&identity.tls_fingerprint()?),
            addresses: Vec::new(),
        };
        tokio::spawn(advertise::run(Identity::default_dir()?, config));
    }
    let mut capture = capture;
    let compositor = capture.compositor.take();
    let input = capture.input.clone();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    // `systemctl --user reload` (SIGHUP): re-read passwords (M9).
    tokio::spawn({
        let agent = Arc::clone(&agent);
        async move {
            while hangup.recv().await.is_some() {
                match load_logins_blocking(KeyringFailure::Abort).await {
                    Ok(logins) => agent.reload_logins(logins),
                    Err(error) => {
                        warn!(error = %format!("{error:#}"), "could not reload passwords");
                    }
                }
            }
        }
    });
    let pump = tokio::spawn(Arc::clone(&agent).pump(capture));
    let result = tokio::select! {
        result = Arc::clone(&agent).listen(listener) => result,
        result = pump => result.context("capture task").and_then(|r| r),
        _ = tokio::signal::ctrl_c() => Ok(()),
        _ = terminate.recv() => Ok(()),
    };
    capture::shutdown(&input, compositor).await;
    result
}

/// [`load_logins`] off the async workers: the OS keyring client blocks (and
/// starts its own runtime), which must not happen on a runtime thread.
async fn load_logins_blocking(keyring: KeyringFailure) -> Result<runtime::Logins> {
    tokio::task::spawn_blocking(move || load_logins(keyring))
        .await
        .context("loading passwords")?
}

/// What an unreadable OS keyring means for the VNC password.
#[derive(Clone, Copy)]
enum KeyringFailure {
    /// At startup: carry on without it (warn).
    Continue,
    /// On reload: abort, keeping the current logins (fail closed: never
    /// drop a password just because the keyring is locked right now).
    Abort,
}

/// Reads who may log in: agent users (Argon2id) and the legacy VNC password.
fn load_logins(keyring: KeyringFailure) -> Result<runtime::Logins> {
    let credentials = AgentCredentials::load(AgentCredentials::default_path()?)
        .context("reading agent credentials")?;
    let vnc_password = match credentials.vncauth_password(&KeyringKeySource::agent()) {
        Ok(password) => password,
        Err(error) => match keyring {
            KeyringFailure::Continue => {
                warn!(%error, "the legacy VncAuth password is unavailable");
                None
            }
            KeyringFailure::Abort => {
                return Err(error).context("the OS keyring can't be read right now");
            }
        },
    };
    let has_users = credentials.usernames().next().is_some();
    let verifier =
        has_users.then(|| Arc::new(CredentialsVerifier(credentials)) as Arc<dyn UserVerifier>);
    Ok(runtime::Logins {
        verifier,
        vnc_password,
    })
}

/// The listening socket: the one systemd passed (socket activation, M7) or
/// a new one. With a passed socket, its address decides loopback vs. LAN.
async fn open_listener(
    mut passed: listenfd::ListenFd,
    listen: Listen,
    port: u16,
) -> Result<(TcpListener, Listen)> {
    if passed.len() > 1 {
        bail!(
            "systemd passed {} sockets; use one ListenStream per unit",
            passed.len()
        );
    }
    if let Some(socket) = passed
        .take_tcp_listener(0)
        .context("using the socket passed by systemd")?
    {
        socket.set_nonblocking(true)?;
        let listener = TcpListener::from_std(socket)?;
        let ip = listener.local_addr()?.ip();
        let mode = if ip.is_loopback() {
            Listen::Loopback
        } else {
            Listen::Lan
        };
        return Ok((listener, mode));
    }
    Ok((bind(listen, port).await?, listen))
}

/// Headless settings. Without `--display`, a socket-activated agent on port
/// 5901–5999 uses display `:1`–`:99`, like `vncserver`.
fn headless_setup(
    options: &AgentOptions,
    headless: bool,
    port: u16,
) -> Result<capture::HeadlessSetup> {
    let display = options
        .display
        .or_else(|| (headless && (5901..=5999).contains(&port)).then(|| u32::from(port - 5900)));
    Ok(capture::HeadlessSetup {
        x11_display: options.x11_display.clone(),
        session_umask: None,
        width: options.geometry.0,
        height: options.geometry.1,
        display,
        startup: if headless {
            Some(startup_script(options.startup.clone())?)
        } else {
            None
        },
    })
}

async fn bind(listen: Listen, port: u16) -> Result<TcpListener> {
    let address = match listen {
        Listen::Loopback => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        Listen::Lan => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port),
    };
    match TcpListener::bind(address).await {
        Ok(listener) => Ok(listener),
        // No IPv6 on this machine: fall back to IPv4 only.
        Err(_) if listen == Listen::Lan => {
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port))
                .await
                .with_context(|| format!("listening on port {port}"))
        }
        Err(error) => Err(error).with_context(|| format!("listening on port {port}")),
    }
}

fn show_identity(identity: &Identity) -> Result<()> {
    println!("Viewers connecting to this computer will see one of these fingerprints:\n");
    println!(
        "TLS certificate (VeNCrypt):\n{}\n",
        format_fingerprint(&identity.tls_fingerprint()?)
    );
    println!(
        "RSA key (RSA-AES):\n{}",
        format_fingerprint(&identity.ra2.fingerprint())
    );
    Ok(())
}

/// U-3: approval once per login. The token lives in `$XDG_RUNTIME_DIR`,
/// which the session clears at logout, so a new login asks again.
fn portal_token_file() -> std::path::PathBuf {
    let token_file = std::env::var_os("XDG_RUNTIME_DIR")
        .map_or_else(std::env::temp_dir, std::path::PathBuf::from)
        .join("farsight")
        .join(identity::PORTAL_TOKEN_FILE);
    if let Some(dir) = token_file.parent() {
        use std::os::unix::fs::DirBuilderExt;
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir);
    }
    token_file
}

/// The headless startup script (U-10): the one given, or
/// `~/.config/farsight/headless-startup`, created with the default contents
/// (a terminal and a file manager) the first time so it can be edited.
fn startup_script(given: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(path) = given {
        check_startup_script(&path)?;
        return Ok(path);
    }
    let path = Identity::default_dir()?
        .parent()
        .map(|config| config.join("headless-startup"))
        .context("no config folder")?;
    if !path.exists() {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        file.write_all(farsight_headless::DEFAULT_STARTUP_SCRIPT.as_bytes())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        info!(script = %path.display(), "created the headless startup script; edit it to choose your apps");
    }
    check_startup_script(&path)?;
    Ok(path)
}

/// The startup script runs as you every time the desktop starts, so it must
/// be yours and writable only by you (tamper check, T1037).
fn check_startup_script(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).with_context(|| format!("reading {}", path.display()))?;
    let mine = meta.uid() == rustix::process::geteuid().as_raw();
    if !mine || meta.mode() & 0o022 != 0 {
        bail!(
            "{} must belong to you and be writable only by you (try `chmod go-w`)",
            path.display()
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The OS account running the agent: logins as this user skip the prompt (U-2).
fn owner() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .filter(|u| !u.is_empty())
}

fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "farsight".into())
}

/// Defaults from `~/.config/farsight/agent/settings` (written by the
/// Farsight Sharing app); missing or unreadable means the built-in ones.
/// The live desktop's defaults from the Farsight Sharing settings file.
/// Network access chosen there is logged, so it never goes unnoticed.
fn settings_file_defaults() -> AgentOptions {
    let Some(path) = Identity::default_dir().ok().map(|dir| dir.join("settings")) else {
        return AgentOptions::default();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return AgentOptions::default();
    };
    let defaults = options::settings_defaults(&text);
    if defaults.listen == Listen::Lan {
        info!(
            "{}: listen = lan, so computers on the network may connect",
            path.display()
        );
    }
    defaults
}
