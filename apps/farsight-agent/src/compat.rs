//! TigerVNC compatibility names (roadmap M9, user decision U-11): when
//! installed as `x0vncserver`, `w0vncserver` or `vncserver` (the optional
//! `farsight-tigervnc-compat` package), the agent accepts TigerVNC's options.

use std::path::Path;

use crate::options::{AgentOptions, Backend, Listen, UsageError};

/// Which program the agent was started as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Personality {
    Farsight,
    /// Share the live X11 desktop (`x0vncserver`).
    X0vncserver,
    /// Share the live Wayland desktop (`w0vncserver`, TigerVNC 1.15+).
    W0vncserver,
    /// Start or stop headless desktops (`vncserver`).
    Vncserver,
}

impl Personality {
    /// From `argv[0]`.
    pub fn from_program(program: &str) -> Self {
        match Path::new(program).file_name().and_then(|n| n.to_str()) {
            Some("x0vncserver") => Self::X0vncserver,
            Some("w0vncserver") => Self::W0vncserver,
            Some("vncserver") => Self::Vncserver,
            _ => Self::Farsight,
        }
    }
}

/// TigerVNC options that change nothing here (Farsight picks secure
/// settings itself); accepted so existing scripts keep working.
const IGNORED: [&str; 17] = [
    "alwaysshared",
    "nevershared",
    "disconnectclients",
    "maxprocessorusage",
    "pollinginterval",
    "depth",
    "log",
    "acceptcuttext",
    "sendcuttext",
    "maxidletime",
    "maxconnectiontime",
    "maxdisconnectiontime",
    "remapkeys",
    "useipv6",
    "useipv4",
    "zliblevel",
    "querytimeout",
];

/// Ignored options that concern security: worth a warning, not a note.
const SECURITY_RELEVANT: [&str; 3] = ["securitytypes", "queryconnect", "rfbunixpath"];

/// What `x0vncserver`/`w0vncserver` should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrapingCommand {
    Run(AgentOptions, Option<String>),
    Help,
}

/// Parses `x0vncserver`/`w0vncserver` options (`-Name value`, `-Name=value`,
/// case-insensitive). Returns the agent options and the X display to open.
///
/// # Errors
/// [`UsageError`] for unknown or unsupported options.
pub fn scraping_options(
    personality: Personality,
    args: &[String],
) -> Result<ScrapingCommand, UsageError> {
    let mut options = AgentOptions {
        backend: if personality == Personality::W0vncserver {
            Backend::Wayland
        } else {
            Backend::X11
        },
        ..AgentOptions::default()
    };
    let mut display = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (name, inline) = split(arg);
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| UsageError(format!("{arg} needs a value")))
        };
        match name.as_str() {
            "help" | "h" | "?" => return Ok(ScrapingCommand::Help),
            "display" => display = Some(value()?),
            "rfbport" => {
                let v = value()?;
                options.port = v
                    .parse()
                    .map_err(|_| UsageError(format!("invalid port {v:?}")))?;
            }
            "localhost" => {
                options.listen = if boolean(inline.as_deref())? {
                    Listen::Loopback
                } else {
                    Listen::Lan
                };
            }
            "acceptkeyevents" | "acceptpointerevents" => {
                if !boolean(inline.as_deref())? {
                    options.view_only = true;
                }
            }
            "passwordfile" | "rfbauth" | "passwd" => {
                return Err(UsageError(
                    "Farsight doesn't read vncpasswd files; run `farsight-passwd import` once instead".into(),
                ));
            }
            "geometry" => {
                return Err(UsageError(
                    "sharing part of the screen (-Geometry) isn't supported".into(),
                ));
            }
            // TigerVNC users pick loopback or the network this way. Binding
            // to one particular address isn't supported: refuse rather than
            // listen more widely than asked.
            "interface" => {
                let address = value()?;
                options.listen = match address.as_str() {
                    "127.0.0.1" | "::1" | "localhost" => Listen::Loopback,
                    "0.0.0.0" | "::" => Listen::Lan,
                    other => {
                        return Err(UsageError(format!(
                            "-interface {other}: listening on one particular address isn't supported \
                             (use 127.0.0.1, or 0.0.0.0 for every network)"
                        )));
                    }
                };
            }
            other if SECURITY_RELEVANT.contains(&other) => {
                if inline.is_none() && other != "queryconnect" {
                    let _ = args.next();
                }
                tracing::warn!(
                    option = %arg,
                    "ignored: Farsight chooses security itself (encrypted types; loopback by default)"
                );
            }
            other if IGNORED.contains(&other) => {
                if inline.is_none() && takes_value(other) {
                    let _ = args.next();
                }
                tracing::info!(option = %arg, "TigerVNC option accepted but not needed by Farsight");
            }
            _ => return Err(UsageError(format!("unknown option {arg}"))),
        }
    }
    Ok(ScrapingCommand::Run(options, display))
}

/// What `vncserver` should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VncserverCommand {
    Start {
        display: Option<u32>,
        geometry: (u16, u16),
        listen: Listen,
    },
    Kill(u32),
    List,
    Help,
}

/// Defaults from TigerVNC's `vncserver` config file (`geometry=WxH`,
/// `localhost` / `localhost=no`; `#` comments). Other settings are ignored:
/// Farsight chooses security itself.
pub fn vncserver_config(text: &str) -> ((u16, u16), Listen) {
    let mut geometry = AgentOptions::default().geometry;
    let mut listen = Listen::Loopback;
    for line in text.lines().map(str::trim) {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .map_or((line, None), |(n, v)| (n.trim(), Some(v.trim())));
        match name.to_ascii_lowercase().as_str() {
            "geometry" => {
                if let Some(size) = value.and_then(|v| parse_geometry(v).ok()) {
                    geometry = size;
                }
            }
            "localhost" => {
                if let Ok(local) = boolean(value) {
                    listen = if local { Listen::Loopback } else { Listen::Lan };
                }
            }
            _ => {}
        }
    }
    (geometry, listen)
}

/// The `localhost` setting of TigerVNC's administrator file
/// (`vncserver-config-mandatory`), which wins over the user's choices.
pub fn mandatory_listen(text: &str) -> Option<Listen> {
    text.lines().map(str::trim).find_map(|line| {
        let (name, value) = line
            .split_once('=')
            .map_or((line, None), |(n, v)| (n.trim(), Some(v.trim())));
        (name.eq_ignore_ascii_case("localhost"))
            .then(|| boolean(value).ok())
            .flatten()
            .map(|local| if local { Listen::Loopback } else { Listen::Lan })
    })
}

/// Parses `vncserver [:N] [-geometry WxH] [-localhost yes|no]`,
/// `vncserver -kill :N` and `vncserver -list`.
///
/// # Errors
/// [`UsageError`] for unknown options or a bad display/geometry.
pub fn vncserver_options(
    args: &[String],
    defaults: ((u16, u16), Listen),
) -> Result<VncserverCommand, UsageError> {
    let mut display = None;
    let (mut geometry, mut listen) = defaults;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg.starts_with(':') {
            display = Some(parse_display(arg)?);
            continue;
        }
        let (name, inline) = split(arg);
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| UsageError(format!("{arg} needs a value")))
        };
        match name.as_str() {
            "help" | "h" | "?" => return Ok(VncserverCommand::Help),
            "list" => return Ok(VncserverCommand::List),
            "kill" => return Ok(VncserverCommand::Kill(parse_display(&value()?)?)),
            "geometry" => geometry = parse_geometry(&value()?)?,
            "localhost" => {
                let v = value()?;
                listen = if boolean(Some(&v))? {
                    Listen::Loopback
                } else {
                    Listen::Lan
                };
            }
            "depth" | "name" | "xstartup" | "fg" | "autokill" => {
                if inline.is_none() && takes_value(&name) {
                    let _ = args.next();
                }
                tracing::info!(option = %arg, "vncserver option accepted but not needed by Farsight");
            }
            _ => return Err(UsageError(format!("unknown option {arg}"))),
        }
    }
    Ok(VncserverCommand::Start {
        display,
        geometry,
        listen,
    })
}

fn split(arg: &str) -> (String, Option<String>) {
    let arg = arg.trim_start_matches('-');
    match arg.split_once('=') {
        Some((name, value)) => (name.to_ascii_lowercase(), Some(value.to_owned())),
        None => (arg.to_ascii_lowercase(), None),
    }
}

fn takes_value(name: &str) -> bool {
    !matches!(
        name,
        "alwaysshared" | "nevershared" | "disconnectclients" | "fg" | "autokill"
    )
}

/// TigerVNC booleans: a bare flag is true; `=0/1/yes/no/true/false/on/off`.
fn boolean(value: Option<&str>) -> Result<bool, UsageError> {
    match value.map(str::to_ascii_lowercase).as_deref() {
        None | Some("1" | "yes" | "true" | "on") => Ok(true),
        Some("0" | "no" | "false" | "off") => Ok(false),
        Some(other) => Err(UsageError(format!("invalid yes/no value {other:?}"))),
    }
}

fn parse_display(text: &str) -> Result<u32, UsageError> {
    text.trim_start_matches(':')
        .parse()
        .ok()
        .filter(|n| (1..=99).contains(n))
        .ok_or_else(|| UsageError(format!("invalid display {text:?} (use :1 to :99)")))
}

fn parse_geometry(text: &str) -> Result<(u16, u16), UsageError> {
    let invalid = || UsageError(format!("invalid geometry {text:?} (use e.g. 1920x1080)"));
    let lower = text.to_ascii_lowercase();
    let (w, h) = lower.split_once('x').ok_or_else(invalid)?;
    let size: (u16, u16) = (
        w.parse().map_err(|_| invalid())?,
        h.parse().map_err(|_| invalid())?,
    );
    if (1..=8192).contains(&size.0) && (1..=8192).contains(&size.1) {
        Ok(size)
    } else {
        Err(invalid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn personalities_come_from_the_program_name() {
        assert_eq!(
            Personality::from_program("/usr/bin/x0vncserver"),
            Personality::X0vncserver
        );
        assert_eq!(
            Personality::from_program("w0vncserver"),
            Personality::W0vncserver
        );
        assert_eq!(
            Personality::from_program("vncserver"),
            Personality::Vncserver
        );
        assert_eq!(
            Personality::from_program("farsight-agent"),
            Personality::Farsight
        );
    }

    #[test]
    fn x0vncserver_options_map_onto_the_agent() {
        let Ok(ScrapingCommand::Run(o, display)) = scraping_options(
            Personality::X0vncserver,
            &args(&[
                "-display",
                ":1",
                "-rfbport=5902",
                "-localhost=0",
                "-AcceptKeyEvents=0",
                "-SecurityTypes",
                "VncAuth",
                "-AlwaysShared",
            ]),
        ) else {
            panic!("expected Run");
        };
        assert_eq!(display.as_deref(), Some(":1"));
        assert_eq!(
            (o.backend, o.port, o.listen, o.view_only),
            (Backend::X11, 5902, Listen::Lan, true)
        );
        let Ok(ScrapingCommand::Run(o, _)) = scraping_options(Personality::W0vncserver, &[]) else {
            panic!("expected Run");
        };
        assert_eq!(
            (o.backend, o.listen),
            (Backend::Wayland, Listen::Loopback),
            "secure default kept"
        );
        assert!(
            scraping_options(Personality::X0vncserver, &args(&["-PasswordFile", "x"])).is_err()
        );
        assert!(scraping_options(Personality::X0vncserver, &args(&["-bogus"])).is_err());
        // Options from real unit files are accepted; -interface picks the network.
        let Ok(ScrapingCommand::Run(o, _)) = scraping_options(
            Personality::X0vncserver,
            &args(&[
                "-Log",
                "*:stderr:30",
                "-AcceptCutText=0",
                "-interface",
                "0.0.0.0",
                "-QueryConnect",
            ]),
        ) else {
            panic!("expected Run");
        };
        assert_eq!(o.listen, Listen::Lan);
        assert!(
            scraping_options(Personality::X0vncserver, &args(&["-interface", "10.8.0.1"])).is_err(),
            "never wider than asked"
        );
        assert_eq!(
            mandatory_listen("# admin\nlocalhost\n"),
            Some(Listen::Loopback)
        );
        assert_eq!(mandatory_listen("geometry=800x600\n"), None);
    }

    fn vncserver_options_d(args: &[String]) -> Result<VncserverCommand, UsageError> {
        vncserver_options(args, ((1920, 1080), Listen::Loopback))
    }

    #[test]
    fn vncserver_config_files_give_defaults() {
        let config = "# my settings\ngeometry=1280x720\nlocalhost=no\nsecuritytypes=vncauth\n";
        assert_eq!(vncserver_config(config), ((1280, 720), Listen::Lan));
        assert_eq!(
            vncserver_config("localhost\n"),
            ((1920, 1080), Listen::Loopback)
        );
        let Ok(VncserverCommand::Start {
            geometry, listen, ..
        }) = vncserver_options(&args(&["-localhost", "yes"]), ((1280, 720), Listen::Lan))
        else {
            panic!("expected Start");
        };
        assert_eq!(
            (geometry, listen),
            ((1280, 720), Listen::Loopback),
            "options win"
        );
    }

    #[test]
    fn vncserver_commands_parse() {
        assert_eq!(
            vncserver_options_d(&args(&[
                ":3",
                "-geometry",
                "1280x800",
                "-localhost",
                "no",
                "-depth",
                "24"
            ])),
            Ok(VncserverCommand::Start {
                display: Some(3),
                geometry: (1280, 800),
                listen: Listen::Lan
            })
        );
        assert_eq!(
            vncserver_options_d(&args(&["-kill", ":2"])),
            Ok(VncserverCommand::Kill(2))
        );
        assert_eq!(
            vncserver_options_d(&args(&["-list"])),
            Ok(VncserverCommand::List)
        );
        assert!(matches!(
            vncserver_options_d(&[]),
            Ok(VncserverCommand::Start {
                display: None,
                listen: Listen::Loopback,
                ..
            })
        ));
        assert!(vncserver_options_d(&args(&[":0"])).is_err());
        assert!(vncserver_options_d(&args(&["-geometry", "huge"])).is_err());
    }
}
