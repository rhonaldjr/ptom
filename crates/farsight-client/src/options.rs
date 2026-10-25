//! TigerVNC-compatible viewer command line.
//!
//! Accepts `-Name value`, `-Name=value`, `--Name=value` and bare boolean
//! flags (`-ViewOnly`, `-ViewOnly=0`), case-insensitively, plus one
//! positional server address.

use std::path::PathBuf;

use thiserror::Error;

use crate::ServerAddress;
use crate::address::AddressError;

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Start the viewer.
    Run(ViewerOptions),
    /// Print usage.
    Help,
    /// Print the version.
    Version,
    /// Download Cisco's OpenH264 library (`--install-openh264`), then exit.
    InstallOpenH264,
}

/// Viewer settings from the command line.
#[allow(clippy::struct_excessive_bools)] // Independent user settings, mirroring TigerVNC flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerOptions {
    /// Server to connect to; `None` opens the connection dialog.
    pub server: Option<ServerAddress>,
    /// Do not send keyboard, pointer or clipboard input.
    pub view_only: bool,
    /// Start in full-screen mode.
    pub full_screen: bool,
    /// Ask the server to keep other viewers connected.
    pub shared: bool,
    /// JPEG quality 0–9 for Tight.
    pub quality: Option<u8>,
    /// Compression level 0–9.
    pub compress: Option<u8>,
    /// Never use lossy JPEG.
    pub no_jpeg: bool,
    /// Accept Open H.264 video (`-H264`, Farsight extension, M10).
    pub h264: bool,
    /// Allow unencrypted, legacy security without asking (Farsight extension).
    pub allow_insecure: bool,
    /// Legacy `vncpasswd` file to read the password from (read-only, ADR-0014).
    pub password_file: Option<PathBuf>,
    /// Key that opens the viewer menu.
    pub menu_key: String,
    /// Resize the remote desktop to fit the window.
    pub remote_resize: bool,
    /// Ask the server for this desktop size on connecting (`-DesktopSize`).
    pub desktop_size: Option<(u16, u16)>,
}

impl Default for ViewerOptions {
    fn default() -> Self {
        Self {
            server: None,
            view_only: false,
            full_screen: false,
            shared: true,
            quality: None,
            compress: None,
            no_jpeg: false,
            h264: false,
            allow_insecure: false,
            password_file: None,
            menu_key: "F8".into(),
            remote_resize: true,
            desktop_size: None,
        }
    }
}

/// Why the command line was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OptionsError {
    /// An option name Farsight doesn't know.
    #[error("unknown option -{0} (try --help)")]
    Unknown(String),
    /// An option that needs a value was last on the line.
    #[error("option -{0} needs a value")]
    MissingValue(String),
    /// A value of the wrong type.
    #[error("invalid value {value:?} for -{option}")]
    InvalidValue {
        /// The option.
        option: String,
        /// What was given.
        value: String,
    },
    /// More than one server address.
    #[error("more than one server given: {0}")]
    ExtraArgument(String),
    /// The server address is malformed.
    #[error(transparent)]
    Address(#[from] AddressError),
}

/// Usage text.
pub const USAGE: &str = "\
Usage: farsight [options] [host][:display | ::port]

Options (TigerVNC-compatible; case-insensitive; -Name value or -Name=value):
  -ViewOnly             Don't send keyboard, mouse or clipboard input
  -FullScreen           Start full screen
  -Shared=0             Ask the server to disconnect other viewers
  -QualityLevel N       JPEG quality 0-9
  -CompressLevel N      Compression level 0-9
  -NoJPEG               Never use lossy JPEG (exact pixels, more bandwidth)
  -H264                 Accept H.264 video (needs --install-openh264 once)
  --install-openh264    Download Cisco's OpenH264 library, then exit
  -PasswordFile FILE    Read the password from a vncpasswd file (alias -passwd)
  -MenuKey KEY          Key that opens the viewer menu (default F8)
  -RemoteResize=0       Don't resize the remote desktop to fit the window
  -DesktopSize=WxH      Ask the server for this desktop size (e.g. 2560x1440);
                        the F8 menu changes it while connected
  -AllowInsecure        Connect without encryption without asking
  -help, --version
";

impl Command {
    /// Parses arguments (without the program name).
    ///
    /// # Errors
    /// [`OptionsError`] for unknown options, missing or invalid values, or a
    /// malformed address.
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, OptionsError> {
        Self::parse_with_defaults(ViewerOptions::default(), args)
    }

    /// Like [`Self::parse`], starting from `defaults` (e.g. TigerVNC's saved
    /// settings, see [`tigervnc_defaults`]); the command line wins.
    ///
    /// # Errors
    /// As for [`Self::parse`].
    pub fn parse_with_defaults<I: IntoIterator<Item = String>>(
        defaults: ViewerOptions,
        args: I,
    ) -> Result<Self, OptionsError> {
        let mut options = defaults;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let Some(flag) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
                if options.server.is_some() {
                    return Err(OptionsError::ExtraArgument(arg));
                }
                options.server = Some(ServerAddress::parse(&arg)?);
                continue;
            };
            let (name, inline) = match flag.split_once('=') {
                Some((name, value)) => (name.to_ascii_lowercase(), Some(value.to_owned())),
                None => (flag.to_ascii_lowercase(), None),
            };
            match name.as_str() {
                "help" | "h" | "?" => return Ok(Self::Help),
                "version" => return Ok(Self::Version),
                "install-openh264" => return Ok(Self::InstallOpenH264),
                "h264" => options.h264 = boolean(&name, inline.as_deref())?,
                "viewonly" => options.view_only = boolean(&name, inline.as_deref())?,
                "fullscreen" => options.full_screen = boolean(&name, inline.as_deref())?,
                "shared" => options.shared = boolean(&name, inline.as_deref())?,
                "remoteresize" => options.remote_resize = boolean(&name, inline.as_deref())?,
                "allowinsecure" => options.allow_insecure = boolean(&name, inline.as_deref())?,
                "nojpeg" => options.no_jpeg = boolean(&name, inline.as_deref())?,
                "qualitylevel" => {
                    options.quality = Some(level(&name, &value(&name, inline, &mut args)?)?);
                }
                "compresslevel" => {
                    options.compress = Some(level(&name, &value(&name, inline, &mut args)?)?);
                }
                "passwordfile" | "passwd" => {
                    options.password_file = Some(PathBuf::from(value(&name, inline, &mut args)?));
                }
                "menukey" => options.menu_key = value(&name, inline, &mut args)?,
                "desktopsize" => {
                    let text = value(&name, inline, &mut args)?;
                    options.desktop_size = Some(size(&text).ok_or(OptionsError::InvalidValue {
                        option: name.clone(),
                        value: text,
                    })?);
                }
                _ => return Err(OptionsError::Unknown(name)),
            }
        }
        Ok(Self::Run(options))
    }
}

fn value(
    name: &str,
    inline: Option<String>,
    rest: &mut impl Iterator<Item = String>,
) -> Result<String, OptionsError> {
    inline
        .or_else(|| rest.next())
        .ok_or_else(|| OptionsError::MissingValue(name.to_owned()))
}

/// Viewer defaults from a TigerVNC viewer settings file (`default.tigervnc`:
/// a header line, then `Name=value` lines). Only settings Farsight has are
/// read; unknown or invalid lines are skipped, and the saved server address
/// is not used (Farsight never connects by itself).
#[must_use]
pub fn tigervnc_defaults(text: &str) -> ViewerOptions {
    let mut options = ViewerOptions::default();
    // TigerVNC saves every setting after each connection; with AutoSelect on
    // (its default) the levels it saved weren't the user's choice.
    let auto_select = !text.lines().any(|l| {
        l.split_once('=')
            .is_some_and(|(n, v)| n.trim().eq_ignore_ascii_case("autoselect") && v.trim() == "0")
    });
    for line in text.lines().skip_while(|l| l.starts_with("TigerVNC")) {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        let flag = boolean(&name, Some(value)).ok();
        match name.as_str() {
            "viewonly" => options.view_only = flag.unwrap_or(options.view_only),
            "fullscreen" => options.full_screen = flag.unwrap_or(options.full_screen),
            "shared" => options.shared = flag.unwrap_or(options.shared),
            "remoteresize" => options.remote_resize = flag.unwrap_or(options.remote_resize),
            "nojpeg" => options.no_jpeg = flag.unwrap_or(options.no_jpeg),
            "qualitylevel" if !auto_select => {
                options.quality = level(&name, value).ok().or(options.quality);
            }
            "compresslevel" if !auto_select => {
                options.compress = level(&name, value).ok().or(options.compress);
            }
            "menukey" if !value.is_empty() => value.clone_into(&mut options.menu_key),
            "desktopsize" => options.desktop_size = size(value).or(options.desktop_size),
            _ => {}
        }
    }
    options
}

/// `WxH`, each side 1–16384.
fn size(text: &str) -> Option<(u16, u16)> {
    let (w, h) = text.trim().split_once(['x', 'X'])?;
    let side = |s: &str| {
        s.trim()
            .parse::<u16>()
            .ok()
            .filter(|n| (1..=16_384).contains(n))
    };
    Some((side(w)?, side(h)?))
}

fn boolean(name: &str, inline: Option<&str>) -> Result<bool, OptionsError> {
    match inline.map(str::to_ascii_lowercase).as_deref() {
        None | Some("1" | "true" | "yes" | "on") => Ok(true),
        Some("0" | "false" | "no" | "off") => Ok(false),
        Some(other) => Err(OptionsError::InvalidValue {
            option: name.to_owned(),
            value: other.to_owned(),
        }),
    }
}

fn level(name: &str, value: &str) -> Result<u8, OptionsError> {
    match value.parse::<u8>() {
        Ok(n) if n <= 9 => Ok(n),
        _ => Err(OptionsError::InvalidValue {
            option: name.to_owned(),
            value: value.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn desktop_size_is_parsed_and_checked() {
        let Ok(Command::Run(o)) = parse(&["-DesktopSize=2560x1440"]) else {
            panic!("expected Run");
        };
        assert_eq!(o.desktop_size, Some((2560, 1440)));
        assert!(parse(&["-DesktopSize", "big"]).is_err());
        assert!(parse(&["-DesktopSize=0x10"]).is_err());
        assert_eq!(
            tigervnc_defaults("DesktopSize=3840x2160\n").desktop_size,
            Some((3840, 2160))
        );
    }

    #[test]
    fn tigervnc_settings_become_defaults_and_the_command_line_wins() {
        let saved = "TigerVNC Configuration file Version 1.0\n\
                     ServerName=office:1\nAutoSelect=0\nQualityLevel=3\nViewOnly=1\nNoJPEG=0\n\
                     MenuKey=F9\nCompressLevel=nine\nGarbage\n";
        let defaults = tigervnc_defaults(saved);
        assert_eq!(defaults.quality, Some(3));
        assert!(defaults.view_only);
        assert_eq!(defaults.menu_key, "F9");
        assert_eq!(defaults.compress, None, "invalid values are skipped");
        assert!(defaults.server.is_none(), "never connects by itself");
        let Ok(Command::Run(o)) =
            Command::parse_with_defaults(defaults, ["-QualityLevel=7".to_owned()])
        else {
            panic!("expected Run");
        };
        assert_eq!((o.quality, o.view_only), (Some(7), true));
        // What TigerVNC saves by default: AutoSelect on, so its levels are ignored.
        let auto = tigervnc_defaults(
            "TigerVNC Configuration file Version 1.0\nAutoSelect=1\nQualityLevel=8\nCompressLevel=2\n",
        );
        assert_eq!((auto.quality, auto.compress), (None, None));
    }
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, OptionsError> {
        Command::parse(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn tigervnc_style_options() {
        let Ok(Command::Run(o)) = parse(&[
            "-ViewOnly",
            "-fullscreen=1",
            "-QualityLevel",
            "7",
            "--Shared=0",
            "pop-os:1",
        ]) else {
            panic!("expected Run");
        };
        assert!(o.view_only && o.full_screen && !o.shared);
        assert_eq!(o.quality, Some(7));
        assert_eq!(
            o.server,
            Some(ServerAddress {
                host: "pop-os".into(),
                port: 5901
            })
        );
    }

    #[test]
    fn help_version_and_errors() {
        assert_eq!(parse(&["--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-version"]), Ok(Command::Version));
        assert_eq!(
            parse(&["-Bogus"]),
            Err(OptionsError::Unknown("bogus".into()))
        );
        assert_eq!(
            parse(&["-QualityLevel"]),
            Err(OptionsError::MissingValue("qualitylevel".into()))
        );
        assert!(matches!(
            parse(&["-QualityLevel=12"]),
            Err(OptionsError::InvalidValue { .. })
        ));
        assert!(matches!(
            parse(&["a", "b"]),
            Err(OptionsError::ExtraArgument(_))
        ));
    }

    #[test]
    fn no_arguments_opens_the_dialog() {
        assert_eq!(parse(&[]), Ok(Command::Run(ViewerOptions::default())));
    }
}
