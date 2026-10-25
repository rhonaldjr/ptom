//! Command-line parsing. Hand-written: the grammar is small, and it lets us
//! refuse anything that looks like a password on the command line.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
farsight-passwd: manage who may connect to this computer's Farsight agent

Usage:
  farsight-passwd user add <name>     Add a user, or change a user's password
  farsight-passwd user remove <name>  Remove a user
  farsight-passwd user list           List users
  farsight-passwd vncauth set         Set the legacy VNC Authentication password (weak)
  farsight-passwd vncauth clear       Remove the VNC Authentication password
  farsight-passwd import [PATH]       Import a vncpasswd file (default: ~/.vnc/passwd)

Options:
  --stdin        Read the password from standard input (one line) instead of
                 asking. Only for `user add` and `vncauth set`.
  --file PATH    Use this credentials file instead of
                 $XDG_CONFIG_HOME/farsight/agent-credentials.toml
  --delete-old   With `import`: delete the old file afterwards, without asking
  --keep-old     With `import`: keep the old file, without asking
  -h, --help     Show this help
  -V, --version  Show the version

Passwords are never accepted as command-line arguments, because other users
of this computer can see those. farsight-passwd asks for them instead.
";

/// `vncpasswd` compatibility (U-11): what TigerVNC's `vncpasswd` arguments
/// mean for Farsight, as `farsight-passwd` arguments.
///
/// # Errors
/// [`UsageError`] for `-f`, which would produce the weak legacy file format
/// that Farsight never reads (ADR-0014).
pub fn vncpasswd_args(args: &[OsString]) -> Result<Vec<OsString>, UsageError> {
    if args.iter().any(|a| a == "-f") {
        return Err(UsageError(
            "Farsight doesn't use ~/.vnc/passwd files, so `vncpasswd -f` has nothing to make.\n\
             To set the VNC password from a script: `farsight-passwd vncauth set --stdin`."
                .into(),
        ));
    }
    if args
        .iter()
        .any(|a| a == "-h" || a == "--help" || a == "-help")
    {
        return Ok(vec!["--help".into()]);
    }
    // `vncpasswd [file]` and `vncpasswd -view…`: set the VNC password (in
    // Farsight's encrypted store; a file argument is not written to).
    if !args.is_empty() {
        eprintln!(
            "vncpasswd (Farsight): there is no separate view-only password, and no file is written. \
             For a watch-only viewer, give it its own account and run the agent with --view-only."
        );
    }
    Ok(vec!["vncauth".into(), "set".into()])
}

/// What to do with the old file after `import`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldFile {
    Ask,
    Delete,
    Keep,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    UserAdd(String),
    UserRemove(String),
    UserList,
    VncauthSet,
    VncauthClear,
    Import {
        path: Option<PathBuf>,
        old_file: OldFile,
    },
}

/// Where passwords come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordInput {
    Terminal,
    Stdin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub password_input: PasswordInput,
    pub file: Option<PathBuf>,
}

/// A mistake in how the program was called.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

const NO_PASSWORDS_IN_ARGS: &str = "Passwords are never taken from the command line; \
     farsight-passwd will ask for the password (or use --stdin).";

fn usage(message: impl Into<String>) -> UsageError {
    UsageError(message.into())
}

#[derive(Default)]
struct Flags {
    /// `--help` or `--version`, which override any command.
    early: Option<Command>,
    stdin: bool,
    old_file: Option<OldFile>,
    file: Option<PathBuf>,
}

impl Flags {
    fn set_early(&mut self, command: Command) {
        // --help wins over --version.
        if self.early != Some(Command::Help) {
            self.early = Some(command);
        }
    }

    fn set_old_file(&mut self, choice: OldFile) -> Result<(), UsageError> {
        match self.old_file {
            Some(previous) if previous != choice => {
                Err(usage("choose either --delete-old or --keep-old, not both"))
            }
            _ => {
                self.old_file = Some(choice);
                Ok(())
            }
        }
    }
}

/// Parses the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, UsageError> {
    let mut flags = Flags::default();
    let mut positional: Vec<String> = Vec::new();
    let mut args = args.into_iter();
    let mut options_done = false;
    while let Some(arg) = args.next() {
        let text = arg
            .into_string()
            .map_err(|_| usage("arguments must be valid UTF-8"))?;
        if options_done || !text.starts_with('-') || text == "-" {
            positional.push(text);
            continue;
        }
        match text.as_str() {
            "--" => options_done = true,
            "-h" | "--help" => flags.set_early(Command::Help),
            "-V" | "--version" => flags.set_early(Command::Version),
            "--stdin" => flags.stdin = true,
            "--delete-old" => flags.set_old_file(OldFile::Delete)?,
            "--keep-old" => flags.set_old_file(OldFile::Keep)?,
            "--file" => {
                let value = args.next().ok_or_else(|| usage("--file needs a path"))?;
                flags.file = Some(PathBuf::from(value));
            }
            other => {
                if let Some(value) = other.strip_prefix("--file=") {
                    flags.file = Some(PathBuf::from(value));
                    continue;
                }
                // Never echo an option's value: it might be a password.
                let name = other.split('=').next().unwrap_or(other);
                if name.to_lowercase().contains("pass") {
                    return Err(usage(format!(
                        "unknown option {name}. {NO_PASSWORDS_IN_ARGS}"
                    )));
                }
                return Err(usage(format!("unknown option {name}")));
            }
        }
    }
    build(flags, &positional)
}

fn build(flags: Flags, positional: &[String]) -> Result<Invocation, UsageError> {
    let command = match flags.early.clone() {
        Some(early) => early,
        None => command(positional, &flags)?,
    };
    let reads_password = matches!(command, Command::UserAdd(_) | Command::VncauthSet);
    if flags.stdin && !reads_password && !matches!(command, Command::Help | Command::Version) {
        return Err(usage(
            "--stdin only applies to `user add` and `vncauth set`",
        ));
    }
    let is_import = matches!(command, Command::Import { .. });
    if flags.old_file.is_some() && !is_import {
        return Err(usage("--delete-old and --keep-old only apply to `import`"));
    }
    Ok(Invocation {
        command,
        password_input: if flags.stdin {
            PasswordInput::Stdin
        } else {
            PasswordInput::Terminal
        },
        file: flags.file,
    })
}

fn command(positional: &[String], flags: &Flags) -> Result<Command, UsageError> {
    let words: Vec<&str> = positional.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => Err(usage("no command given")),
        ["user", "add", name] => Ok(Command::UserAdd((*name).to_owned())),
        ["user", "remove", name] => Ok(Command::UserRemove((*name).to_owned())),
        ["user", "list"] => Ok(Command::UserList),
        ["user", "add" | "remove"] => Err(usage("which user? Give a name, e.g. `user add alice`")),
        ["vncauth", "set"] => Ok(Command::VncauthSet),
        ["user", "add", _, ..] | ["vncauth", "set", ..] => {
            Err(usage(format!("too many arguments. {NO_PASSWORDS_IN_ARGS}")))
        }
        ["vncauth", "clear"] => Ok(Command::VncauthClear),
        ["import", rest @ ..] if rest.len() <= 1 => Ok(Command::Import {
            path: rest.first().map(PathBuf::from),
            old_file: flags.old_file.unwrap_or(OldFile::Ask),
        }),
        ["user", ..] => Err(usage("unknown `user` command; use add, remove or list")),
        ["vncauth", ..] => Err(usage("unknown `vncauth` command; use set or clear")),
        [other, ..] => Err(usage(format!("unknown command {other:?}"))),
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn vncpasswd_arguments_map_to_farsight_ones() {
        let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(vncpasswd_args(&[]).unwrap(), os(&["vncauth", "set"]));
        assert_eq!(
            vncpasswd_args(&os(&["/home/me/.vnc/passwd"])).unwrap(),
            os(&["vncauth", "set"])
        );
        assert_eq!(vncpasswd_args(&os(&["-help"])).unwrap(), os(&["--help"]));
        assert!(vncpasswd_args(&os(&["-f"])).is_err());
    }
    use super::*;

    fn run(args: &[&str]) -> Result<Invocation, UsageError> {
        parse(args.iter().map(OsString::from))
    }

    fn cmd(args: &[&str]) -> Command {
        run(args).unwrap().command
    }

    #[test]
    fn parses_every_command() {
        assert_eq!(
            cmd(&["user", "add", "alice"]),
            Command::UserAdd("alice".into())
        );
        assert_eq!(
            cmd(&["user", "remove", "bob"]),
            Command::UserRemove("bob".into())
        );
        assert_eq!(cmd(&["user", "list"]), Command::UserList);
        assert_eq!(cmd(&["vncauth", "set"]), Command::VncauthSet);
        assert_eq!(cmd(&["vncauth", "clear"]), Command::VncauthClear);
        assert_eq!(
            cmd(&["import"]),
            Command::Import {
                path: None,
                old_file: OldFile::Ask
            }
        );
        assert_eq!(
            cmd(&["import", "/tmp/p", "--keep-old"]),
            Command::Import {
                path: Some("/tmp/p".into()),
                old_file: OldFile::Keep
            }
        );
        assert_eq!(cmd(&["--help"]), Command::Help);
        assert_eq!(cmd(&["user", "add", "x", "-h"]), Command::Help);
        assert_eq!(cmd(&["-V"]), Command::Version);
    }

    #[test]
    fn parses_options() {
        let inv = run(&["--stdin", "user", "add", "alice", "--file", "/x.toml"]).unwrap();
        assert_eq!(inv.password_input, PasswordInput::Stdin);
        assert_eq!(inv.file, Some(PathBuf::from("/x.toml")));
        let inv = run(&["vncauth", "set", "--file=/y.toml"]).unwrap();
        assert_eq!(inv.password_input, PasswordInput::Terminal);
        assert_eq!(inv.file, Some(PathBuf::from("/y.toml")));
        assert_eq!(
            cmd(&["user", "add", "--", "-dash"]),
            Command::UserAdd("-dash".into())
        );
    }

    #[test]
    fn refuses_passwords_on_the_command_line() {
        for args in [
            &["user", "add", "alice", "hunter2"][..],
            &["vncauth", "set", "hunter2"],
            &["user", "add", "alice", "--password", "hunter2"],
            &["vncauth", "set", "--passwd=hunter2"],
        ] {
            let err = run(args).unwrap_err();
            assert!(
                err.0.contains("never taken from the command line"),
                "{args:?}: {err}"
            );
            assert!(!err.0.contains("hunter2"), "must not echo the password");
        }
    }

    #[test]
    fn rejects_misuse() {
        for args in [
            &[][..],
            &["user"],
            &["user", "add"],
            &["user", "rename", "a"],
            &["user", "list", "extra"],
            &["vncauth"],
            &["vncauth", "clear", "extra"],
            &["import", "a", "b"],
            &["import", "--delete-old", "--keep-old"],
            &["user", "list", "--stdin"],
            &["user", "list", "--keep-old"],
            &["frobnicate"],
            &["--file"],
            &["--bogus", "user", "list"],
        ] {
            assert!(run(args).is_err(), "{args:?} should fail");
        }
    }
}
