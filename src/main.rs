//! Native app for reviewing a branch and its working tree.

mod cli;
mod demo;
mod input;
mod model;
mod source;
mod state;
mod view;

use std::{env, process};

use rune::{App, AppSpec, Titlebar, tooling};

use state::State;

#[allow(
    clippy::print_stderr,
    reason = "Report command-line usage and parse failures"
)]
fn main() {
    rune::prewarm_gpu();
    let options = match cli::Options::parse(env::args_os().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n{}", cli::USAGE);
            process::exit(2);
        }
    };
    if options
        .tooling
        .iter()
        .any(|arg| arg == "-h" || arg == "--help")
    {
        eprintln!("{}\n", cli::USAGE);
    }
    let app = App::new(AppSpec {
        on_key: Some(State::key),
        setup: Some(State::setup),
        poll: Some(State::poll),
        families: ("Inter", "ui-monospace"),
        min_size: Some((920.0, 480.0)),
        ..AppSpec::new(
            "Git Viewer",
            (1240.0, 820.0),
            || State::open(None, false),
            view::review,
        )
    })
    .titlebar(Titlebar::new(view::TOOLBAR_HEIGHT))
    .args(options.tooling)
    .init(|| State::open(options.repository, tooling::headless_run().is_some()))
    .on_visibility(State::visibility);
    let handle = app.handle();
    app.on_ready(move |_| {
        handle.update_if(|state, tx| {
            state.request_refresh(tx);
            false
        });
    })
    .run();
}
