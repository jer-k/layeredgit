//! Native app for reviewing a branch and its working tree.

mod cli;
mod demo;
mod desktop;
mod frame;
mod input;
mod instance;
mod model;
mod palette;
mod panels;
mod preferences;
mod repositories;
mod source;
mod state;
mod view;

use std::{env, process::ExitCode};

const APP_NAME: &str = "layeredgit";
const APP_ID: &str = "local.layeredgit";

#[allow(
    clippy::print_stderr,
    reason = "Report command-line usage and launch failures"
)]
fn main() -> ExitCode {
    rune_mac::launch();
    rune::prewarm_gpu();

    let options = match cli::Options::parse(env::args_os().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    if options
        .tooling
        .iter()
        .any(|arg| arg == "-h" || arg == "--help")
    {
        eprintln!("{}\n", cli::USAGE);
    }
    rune::set_app_id(APP_ID);
    match desktop::run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
