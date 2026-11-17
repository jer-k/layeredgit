//! Native app for reviewing a branch against its base, GitHub-PR style.

use rune::{AppSpec, Cx, El, col, text, tok};

#[derive(Default)]
struct State;

fn view(_: &State, cx: &mut Cx<State>) -> El {
    let theme = tok::theme(cx.env().system_dark);
    col()
        .w_full()
        .h_full()
        .center()
        .fill(theme.bg)
        .child(text("Git Viewer").color(theme.text))
}

fn main() {
    rune::run(AppSpec::new(
        "Git Viewer",
        (1000.0, 700.0),
        State::default,
        view,
    ));
}
