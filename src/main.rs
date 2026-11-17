//! Native app for reviewing a branch and its working tree.

mod demo;
mod input;
mod model;
mod state;
mod view;

use rune::AppSpec;

use state::State;

fn main() {
    rune::run(AppSpec {
        on_key: Some(State::key),
        poll: Some(State::reveal_sidebar),
        families: ("Inter", "ui-monospace"),
        min_size: Some((920.0, 480.0)),
        ..AppSpec::new(
            "Git Viewer",
            (1240.0, 820.0),
            || State::new(demo::review()),
            view::review,
        )
    });
}
