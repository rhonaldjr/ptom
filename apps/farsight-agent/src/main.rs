//! `farsight-agent`: shares this desktop with Farsight and other VNC viewers.
//! Composition root (ADR-0018); the protocol lives in `farsight-server`.

mod capture;
mod identity;
mod options;
mod runtime;

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
    let options = match Command::parse(std::env::args().skip(1)) {
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
    match runtime.block_on(run(options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("farsight-agent: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(options: AgentOptions) -> Result<()> {
    if rustix::process::geteuid().is_root() {
        bail!("farsight-agent must not run as root; run it as the desktop user (ADR-0009)");
    }
    let host = host_name();
    let identity = Identity::load_or_create(&Identity::default_dir()?, &host)?;
    if options.show_identity {
        println!("Viewers connecting to this computer will see one of these fingerprints:\n");
        println!(
            "TLS certificate (VeNCrypt):\n{}\n",
            format_fingerprint(&identity.tls_fingerprint()?)
        );
        println!(
            "RSA key (RSA-AES):\n{}",
            format_fingerprint(&identity.ra2.fingerprint())
        );
        return Ok(());
    }

    let credentials = AgentCredentials::load(AgentCredentials::default_path()?)
        .context("reading agent credentials")?;
    let vnc_password = match credentials.vncauth_password(&KeyringKeySource::agent()) {
        Ok(password) => password,
        Err(error) => {
            warn!(%error, "the legacy VncAuth password is unavailable");
            None
        }
    };
    let has_users = credentials.usernames().next().is_some();
    if options.listen == Listen::Lan && !has_users && vnc_password.is_none() {
        bail!(
            "no passwords are set up, so the agent won't accept connections from the network.\n\
             Run `farsight-passwd user add {}` first.",
            owner().unwrap_or_else(|| "<your-user>".into())
        );
    }
    let verifier: Option<Arc<dyn UserVerifier>> =
        has_users.then(|| Arc::new(CredentialsVerifier(credentials)) as Arc<dyn UserVerifier>);

    let backend = options.backend.resolve(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    );
    let token_file = Identity::default_dir()?.join(identity::PORTAL_TOKEN_FILE);
    if options.forget_portal_approval {
        match std::fs::remove_file(&token_file) {
            Ok(()) => println!("Forgot the remembered screen-sharing approval."),
            Err(_) => println!("No screen-sharing approval was remembered."),
        }
        return Ok(());
    }
    let capture = capture::start(backend, &token_file).await?;
    let settings = Settings {
        owner: owner(),
        tls: farsight_net::tls_acceptor(&identity.cert_pem, &identity.key_pem)?,
        ra2: Arc::clone(&identity.ra2),
        verifier,
        vnc_password,
        allow_legacy: options.allow_legacy,
        view_only: options.view_only,
    };
    let name = format!("{}@{host}", owner().unwrap_or_else(|| "desktop".into()));
    let agent = Agent::new(settings, name, capture.input.clone());
    agent.start_presence().await;

    let listener = bind(options.listen, options.port).await?;
    info!(address = %listener.local_addr()?, ?backend, "sharing this desktop");
    // ADR-0010: announce only when the network may connect, with a minimal TXT
    // record (presence + fingerprint). Kept alive for the agent's lifetime.
    let _advertisement = if options.listen == Listen::Lan && options.advertise {
        let config = farsight_discovery::AdvertiseConfig {
            instance_name: host.clone(),
            host_name: host.clone(),
            port: options.port,
            fingerprint: hex(&identity.tls_fingerprint()?),
            addresses: Vec::new(),
        };
        match farsight_discovery::advertise(&config) {
            Ok(advertisement) => Some(advertisement),
            Err(error) => {
                warn!(%error, "could not announce this computer on the network");
                None
            }
        }
    } else {
        None
    };
    let pump = tokio::spawn(Arc::clone(&agent).pump(capture));
    tokio::select! {
        result = Arc::clone(&agent).listen(listener) => result,
        result = pump => result.context("capture task")?,
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
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
