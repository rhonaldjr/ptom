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
    let invocation = match cli::parse(std::env::args_os().skip(1)) {
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
