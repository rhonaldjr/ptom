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
    /// Allow unencrypted, legacy security without asking (Farsight extension).
    pub allow_insecure: bool,
    /// Legacy `vncpasswd` file to read the password from (read-only, ADR-0014).
    pub password_file: Option<PathBuf>,
    /// Key that opens the viewer menu.
    pub menu_key: String,
    /// Resize the remote desktop to fit the window.
    pub remote_resize: bool,
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
            allow_insecure: false,
            password_file: None,
            menu_key: "F8".into(),
            remote_resize: true,
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
  -PasswordFile FILE    Read the password from a vncpasswd file (alias -passwd)
  -MenuKey KEY          Key that opens the viewer menu (default F8)
  -RemoteResize=0       Don't resize the remote desktop to fit the window
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
        let mut options = ViewerOptions::default();
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
