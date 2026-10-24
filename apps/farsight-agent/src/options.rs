//! `farsight-agent` command line.

use std::fmt;

/// Where the desktop comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Pick from the session type.
    Auto,
    /// Live Wayland session via xdg-desktop-portal (M4).
    Wayland,
    /// Live X11 session (M5).
    X11,
}

/// Which addresses to listen on (U-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listen {
    /// This machine only (default, ADR-0009).
    Loopback,
    /// All interfaces; encrypted security types only unless `--allow-legacy`.
    Lan,
}

/// Agent settings.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Independent switches.
pub struct AgentOptions {
    pub backend: Backend,
    pub listen: Listen,
    pub port: u16,
    pub view_only: bool,
    pub allow_legacy: bool,
    pub advertise: bool,
    pub show_identity: bool,
    pub forget_portal_approval: bool,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            backend: Backend::Auto,
            listen: Listen::Loopback,
            port: 5900,
            view_only: false,
            allow_legacy: false,
            advertise: true,
            show_identity: false,
            forget_portal_approval: false,
        }
    }
}

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(AgentOptions),
    Help,
    Version,
}

/// A command-line mistake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (try --help)", self.0)
    }
}

impl std::error::Error for UsageError {}

pub const USAGE: &str = "\
Usage: farsight-agent [options]

Shares this desktop with Farsight and other VNC viewers.

  --backend auto|wayland|x11   Desktop to share (default: from the session type)
  --listen loopback|lan        Accept connections from this computer only (default)
                               or from your network (encrypted connections only)
  --port N, -rfbport N         TCP port (default 5900)
  --view-only                  Viewers can watch but not control
  --allow-legacy               On the network, also allow unencrypted VncAuth (not recommended)
  --no-advertise               Don't announce this computer on the local network
  --show-identity              Print the fingerprints viewers will see, then exit
  --forget-portal-approval     Forget the remembered Wayland screen-sharing approval, then exit
  -h, --help, --version
";

impl Command {
    /// Parses arguments (without the program name).
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, UsageError> {
        let mut o = AgentOptions::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) => (n.to_ascii_lowercase(), Some(v.to_owned())),
                None => (arg.to_ascii_lowercase(), None),
            };
            let mut value = || {
                inline
                    .clone()
                    .or_else(|| args.next())
                    .ok_or_else(|| UsageError(format!("{name} needs a value")))
            };
            match name.trim_start_matches('-') {
                "h" | "help" => return Ok(Self::Help),
                "version" => return Ok(Self::Version),
                "backend" => {
                    o.backend = match value()?.as_str() {
                        "auto" => Backend::Auto,
                        "wayland" => Backend::Wayland,
                        "x11" => Backend::X11,
                        other => return Err(UsageError(format!("unknown backend {other:?}"))),
                    };
                }
                "listen" => {
                    o.listen = match value()?.as_str() {
                        "loopback" | "localhost" => Listen::Loopback,
                        "lan" | "network" => Listen::Lan,
                        other => return Err(UsageError(format!("unknown listen mode {other:?}"))),
                    };
                }
                "port" | "rfbport" => {
                    let v = value()?;
                    o.port = v
                        .parse()
                        .map_err(|_| UsageError(format!("invalid port {v:?}")))?;
                }
                "view-only" | "viewonly" => o.view_only = true,
                "allow-legacy" => o.allow_legacy = true,
                "no-advertise" => o.advertise = false,
                "show-identity" => o.show_identity = true,
                "forget-portal-approval" => o.forget_portal_approval = true,
                _ => return Err(UsageError(format!("unknown option {arg}"))),
            }
        }
        Ok(Self::Run(o))
    }
}

impl Backend {
    /// Resolves `Auto` from `XDG_SESSION_TYPE` / `WAYLAND_DISPLAY`.
    pub fn resolve(self, session_type: Option<&str>, wayland_display: bool) -> Self {
        match self {
            Self::Auto if session_type == Some("wayland") || wayland_display => Self::Wayland,
            Self::Auto => Self::X11,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, UsageError> {
        Command::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn defaults_are_loopback_and_advertising() {
        assert_eq!(parse(&[]), Ok(Command::Run(AgentOptions::default())));
    }

    #[test]
    fn options_parse() {
        let Ok(Command::Run(o)) = parse(&[
            "--listen",
            "lan",
            "-rfbport=5901",
            "--backend=x11",
            "--view-only",
            "--no-advertise",
        ]) else {
            panic!("expected Run");
        };
        assert_eq!(
            (o.listen, o.port, o.backend, o.view_only, o.advertise),
            (Listen::Lan, 5901, Backend::X11, true, false)
        );
        assert!(parse(&["--listen", "everywhere"]).is_err());
        assert!(parse(&["--port"]).is_err());
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
    }

    #[test]
    fn backend_auto_detection() {
        assert_eq!(
            Backend::Auto.resolve(Some("wayland"), false),
            Backend::Wayland
        );
        assert_eq!(Backend::Auto.resolve(Some("x11"), false), Backend::X11);
        assert_eq!(Backend::Auto.resolve(None, true), Backend::Wayland);
        assert_eq!(Backend::X11.resolve(Some("wayland"), true), Backend::X11);
    }
}
