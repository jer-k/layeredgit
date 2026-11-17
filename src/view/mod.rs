//! Native review window and its virtualized diff pane.

mod chrome;
mod diff;
mod theme;

use rune::{Cx, El, col, row};
use rune_kit::Kit;

use crate::state::State;

/// Draws the application in the current system appearance.
pub(crate) fn review(state: &State, cx: &mut Cx<State>) -> El {
    let kit = Kit::neutral(cx.env().system_dark);
    col()
        .key("review")
        .w_full()
        .h_full()
        .fill(kit.c.page)
        .child(chrome::toolbar(state, &kit))
        .child(
            row()
                .key("workspace")
                .grow(1.0)
                .min_h(0.0)
                .child(chrome::sidebar(state, &kit))
                .child(
                    col()
                        .key("review-pane")
                        .grow(1.0)
                        .min_w(0.0)
                        .min_h(0.0)
                        .px(24.0)
                        .child(chrome::legend(&kit))
                        .child(diff::pane(state, &kit)),
                ),
        )
}
