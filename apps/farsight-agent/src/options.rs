//! `farsight-agent` command line.

use std::fmt;

use farsight_server::settings::{Reach, SharingSettings};

/// Where the desktop comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Pick from the session type.
    Auto,
    /// Live Wayland session via xdg-desktop-portal (M4).
    Wayland,
    /// Live X11 session (M5).
    X11,
    /// A new, persistent desktop with no screen (M7, ADR-0006).
    Headless,
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
    /// `--trust-this-network` (true) / `--untrust-this-network` (false).
    pub change_trust: Option<bool>,
    /// Headless desktop size (`--geometry WxH`).
    pub geometry: (u16, u16),
    /// Headless display number (`:N`): X11 `DISPLAY` and port 5900+N.
    pub display: Option<u32>,
    /// Headless startup script (`--startup`); default in the config folder.
    pub startup: Option<std::path::PathBuf>,
    /// X display to share (`x0vncserver -display`); `$DISPLAY` otherwise.
    pub x11_display: Option<String>,
    /// Offer Open H.264 to viewers that support it (`--h264`, M10).
    pub h264: bool,
    /// Download Cisco's OpenH264, then exit (`--install-openh264`).
    pub install_openh264: bool,
    /// Report which H.264 encoder would be used and how fast, then exit.
    pub check_h264: bool,
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
            change_trust: None,
            geometry: (1920, 1080),
            display: None,
            startup: None,
            x11_display: None,
            h264: false,
            install_openh264: false,
            check_h264: false,
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

  --backend auto|wayland|x11|headless
                               Desktop to share (default: from the session type).
                               headless starts a new desktop that keeps running when
                               nobody is connected (like TigerVNC's vncserver)
  --listen loopback|lan        Accept connections from this computer only (default)
                               or from your network (encrypted connections only)
  --port N, -rfbport N         TCP port (default 5900, or 5900+N with --display :N)
  --display :N                 Headless: display number (X11 DISPLAY and port 5900+N)
  --geometry WxH               Headless: desktop size (default 1920x1080)
  --startup FILE               Headless: script that starts your apps
                               (default ~/.config/farsight/headless-startup)
  --view-only                  Viewers can watch but not control
  --allow-legacy               On the network, also allow unencrypted VncAuth (not recommended)
  --no-advertise               Never announce this computer on the local network
  --trust-this-network         Announce this computer on the current network (e.g. at home), then exit.
                               With --listen lan, the agent announces itself only on trusted networks
  --untrust-this-network       Stop announcing on the current network, then exit
  --h264                       Send video as H.264 to viewers that support it: on the GPU
                               when it can (AMD/Intel), otherwise with OpenH264 (run
                               --install-openh264 first). Needs a build with H.264
  --install-openh264           Download Cisco's OpenH264 library, then exit
  --check-h264                 Show which H.264 encoder --h264 would use and test it, then exit
  --show-identity              Print the fingerprints viewers will see, then exit
  --forget-portal-approval     Forget the remembered Wayland screen-sharing approval, then exit
  -h, --help, --version
";

impl Command {
    /// Parses arguments (without the program name).
    #[cfg(test)]
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, UsageError> {
        Self::parse_with_defaults(AgentOptions::default(), args)
    }

    /// Like [`Self::parse`], starting from `defaults` (the settings file
    /// the Farsight Sharing app writes); the command line wins.
    pub fn parse_with_defaults<I: IntoIterator<Item = String>>(
        defaults: AgentOptions,
        args: I,
    ) -> Result<Self, UsageError> {
        let mut o = defaults;
        let mut port_given = false;
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
                        "headless" => Backend::Headless,
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
                    port_given = true;
                }
                "display" => o.display = Some(parse_display(&value()?)?),
                "geometry" => o.geometry = parse_geometry(&value()?)?,
                "startup" => o.startup = Some(value()?.into()),
                "view-only" | "viewonly" => o.view_only = true,
                "allow-legacy" => o.allow_legacy = true,
                "no-advertise" => o.advertise = false,
                "h264" => o.h264 = true,
                "install-openh264" => o.install_openh264 = true,
                "check-h264" => o.check_h264 = true,
                "show-identity" => o.show_identity = true,
                "forget-portal-approval" => o.forget_portal_approval = true,
                "trust-this-network" => o.change_trust = Some(true),
                "untrust-this-network" => o.change_trust = Some(false),
                _ => return Err(UsageError(format!("unknown option {arg}"))),
            }
        }
        if let Some(display) = o.display {
            if o.backend != Backend::Headless {
                return Err(UsageError("--display is for --backend headless".into()));
            }
            if !port_given {
                o.port = u16::try_from(5900 + display)
                    .map_err(|_| UsageError(format!("display :{display} is too large")))?;
            }
        }
        Ok(Self::Run(o))
    }
}

/// Agent defaults from the sharing settings file (format and rules in
/// [`farsight_server::settings`]).
pub fn settings_defaults(text: &str) -> AgentOptions {
    let settings = SharingSettings::parse(text);
    AgentOptions {
        listen: match settings.reach {
            Reach::ThisComputer => Listen::Loopback,
            Reach::Network => Listen::Lan,
        },
        advertise: settings.announce,
        view_only: settings.view_only,
        ..AgentOptions::default()
    }
}

/// Parses the `farsight-agent` command line. The settings file (`settings`,
/// read only when needed) shapes the live desktop only: headless desktops
/// keep their own secure defaults and are opened up per unit.
pub fn parse_farsight(
    settings: impl FnOnce() -> AgentOptions,
    args: &[String],
) -> Result<Command, UsageError> {
    match Command::parse_with_defaults(AgentOptions::default(), args.iter().cloned())? {
        Command::Run(o) if o.backend == Backend::Headless => Ok(Command::Run(o)),
        _ => Command::parse_with_defaults(settings(), args.iter().cloned()),
    }
}

/// `:N` or `N`, like TigerVNC (N up to 99).
fn parse_display(text: &str) -> Result<u32, UsageError> {
    text.trim_start_matches(':')
        .parse()
        .ok()
        .filter(|n| *n <= 99)
        .ok_or_else(|| UsageError(format!("invalid display {text:?} (use :1 to :99)")))
}

/// `WIDTHxHEIGHT`, e.g. `1920x1080`.
fn parse_geometry(text: &str) -> Result<(u16, u16), UsageError> {
    let invalid = || UsageError(format!("invalid geometry {text:?} (use e.g. 1920x1080)"));
    let lower = text.to_ascii_lowercase();
    let (w, h) = lower.split_once('x').ok_or_else(invalid)?;
    let w: u16 = w.parse().map_err(|_| invalid())?;
    let h: u16 = h.parse().map_err(|_| invalid())?;
    if (1..=8192).contains(&w) && (1..=8192).contains(&h) {
        Ok((w, h))
    } else {
        Err(invalid())
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
    fn headless_desktops_ignore_the_sharing_settings() {
        let lan = || settings_defaults("listen = lan\nview-only = yes");
        let args = |list: &[&str]| list.iter().map(|&a| a.to_owned()).collect::<Vec<_>>();
        let Ok(Command::Run(headless)) = parse_farsight(lan, &args(&["--backend", "headless"]))
        else {
            panic!("expected Run");
        };
        assert_eq!(
            (headless.listen, headless.view_only),
            (Listen::Loopback, false)
        );
        let Ok(Command::Run(live)) = parse_farsight(lan, &args(&[])) else {
            panic!("expected Run");
        };
        assert_eq!((live.listen, live.view_only), (Listen::Lan, true));
    }

    #[test]
    fn the_settings_file_gives_defaults_and_the_command_line_wins() {
        let defaults = settings_defaults(
            "# from Farsight Sharing\nlisten = lan\nannounce = no\nbogus\nview-only = yes\n",
        );
        assert_eq!(
            (defaults.listen, defaults.advertise, defaults.view_only),
            (Listen::Lan, false, true)
        );
        let Ok(Command::Run(o)) =
            Command::parse_with_defaults(defaults, ["--listen=loopback".to_owned()])
        else {
            panic!("expected Run");
        };
        assert_eq!(o.listen, Listen::Loopback);
        assert_eq!(
            settings_defaults("listen = everywhere").listen,
            Listen::Loopback,
            "bad values keep the default"
        );
    }

    #[test]
    fn headless_options_follow_tigervnc_conventions() {
        let Ok(Command::Run(o)) = parse(&[
            "--backend",
            "headless",
            "--display",
            ":3",
            "-geometry",
            "1280x800",
        ]) else {
            panic!("expected Run");
        };
        assert_eq!(
            (o.backend, o.display, o.port, o.geometry),
            (Backend::Headless, Some(3), 5903, (1280, 800))
        );
        let Ok(Command::Run(o)) = parse(&["--backend=headless", "--display=2", "--port=6000"])
        else {
            panic!("expected Run");
        };
        assert_eq!(o.port, 6000, "an explicit port wins");
        assert!(parse(&["--display", ":1"]).is_err(), "only for headless");
        assert!(parse(&["--backend=headless", "--display", ":100"]).is_err());
        assert!(parse(&["--backend=headless", "--geometry", "0x10"]).is_err());
        assert!(parse(&["--backend=headless", "--geometry", "big"]).is_err());
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
