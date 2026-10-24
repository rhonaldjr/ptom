//! `farsight`: the viewer. Composition root: parses the TigerVNC-compatible
//! command line and runs the libcosmic UI (ADR-0003). Session logic lives in
//! `farsight-client`, networking in `farsight-net`.

mod app;
mod keymap;
mod nearby;
mod store;
mod tab;
mod views;

use std::process::ExitCode;

use cosmic::app::Settings;
use cosmic::iced::Size;
use farsight_client::options::{Command, USAGE};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let options = match Command::parse(std::env::args().skip(1)) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("farsight {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("farsight: {error}");
            return ExitCode::from(2);
        }
    };

    let settings = Settings::default().size(Size::new(1280.0, 800.0));
    match cosmic::app::run::<app::App>(settings, options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("farsight: {error}");
            ExitCode::FAILURE
        }
    }
}
