//! `farsight-passwd`: manages the Farsight agent's credentials (ADR-0014,
//! SEC-7, UTL-1): user accounts with Argon2id hashes, and an optional,
//! sealed legacy VNC Authentication password. It never writes the legacy
//! `~/.vnc/passwd` format, but can import it.

mod cli;
mod commands;
mod console;

use std::io::{self, Write};
use std::process::ExitCode;

use farsight_vault::agent::AgentCredentials;
use farsight_vault::{Argon2Params, KeyringKeySource};

use crate::cli::{Command, USAGE};
use crate::commands::Context;
use crate::console::SystemConsole;

fn main() -> ExitCode {
    let mut args: Vec<_> = std::env::args_os().collect();
    let program = args.first().and_then(|p| {
        std::path::Path::new(p)
            .file_name()
            .map(std::borrow::ToOwned::to_owned)
    });
    let args = if program.as_deref() == Some(std::ffi::OsStr::new("vncpasswd")) {
        // Called through the TigerVNC compatibility name (U-11).
        match cli::vncpasswd_args(args.get(1..).unwrap_or_default()) {
            Ok(mapped) => {
                eprintln!(
                    "vncpasswd (Farsight): the password is kept in Farsight's encrypted store, \
                     not in ~/.vnc/passwd."
                );
                mapped
            }
            Err(e) => {
                eprintln!("vncpasswd: {e}");
                return ExitCode::from(2);
            }
        }
    } else {
        args.drain(1..).collect()
    };
    let invocation = match cli::parse(args) {
        Ok(invocation) => invocation,
        Err(e) => {
            eprintln!("farsight-passwd: {e}\nRun `farsight-passwd --help` for usage.");
            return ExitCode::from(2);
        }
    };
    match invocation.command {
        Command::Help => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!("farsight-passwd {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    match run(&invocation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("farsight-passwd: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(invocation: &cli::Invocation) -> anyhow::Result<()> {
    let credentials_path = match &invocation.file {
        Some(path) => path.clone(),
        None => AgentCredentials::default_path()?,
    };
    let keys = KeyringKeySource::agent();
    let mut console = SystemConsole::new(invocation.password_input);
    let (mut stdout, mut stderr) = (io::stdout().lock(), io::stderr().lock());
    let mut ctx = Context {
        credentials_path,
        keys: &keys,
        hash_params: Argon2Params::LOGIN_DEFAULT,
        console: &mut console,
        out: &mut stdout,
        err: &mut stderr,
    };
    let result = commands::run(&invocation.command, &mut ctx);
    stdout.flush().ok();
    result
}
