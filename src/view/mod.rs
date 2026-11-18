//! Native review window and its virtualized diff pane.

mod chrome;
mod diff;
mod pulse;
mod theme;

use rune::{Cx, El, col, row, text};
use rune_kit::Kit;

use crate::{
    palette::Palette,
    state::{State, Status},
};

/// Height of the review toolbar and native controls.
pub(crate) const TOOLBAR_HEIGHT: f32 = 68.0;

/// Draws the application in the current system appearance.
pub(crate) fn review(state: &State, cx: &mut Cx<State>) -> El {
    if state.headless_reduced_motion {
        cx.reduced_motion(true);
    }
    let kit = Kit::neutral(cx.env().system_dark);
    cx.caption_color(kit.c.text_base);
    let palette = state.palette(kit.dark);
    let content = match &state.status {
        Status::Loading => notice("Loading review…", None, &kit),
        Status::Failed(error) => notice("Couldn’t load review", Some(error), &kit),
        Status::Ready { refresh_error } => {
            workspace(state, refresh_error.as_deref(), &kit, palette, cx)
        }
    };
    #[cfg(feature = "dev")]
    let content = state.tuning.panel(content, kit.dark);
    col()
        .key("review")
        .w_full()
        .h_full()
        .fill(kit.c.page)
        .child(chrome::toolbar(state, &kit, cx.chrome_insets()))
        .child(content)
}

fn workspace(
    state: &State,
    refresh_error: Option<&str>,
    kit: &Kit,
    palette: Palette,
    cx: &Cx<State>,
) -> El {
    let mut pane = col()
        .key("review-pane")
        .grow(1.0)
        .min_w(0.0)
        .min_h(0.0)
        .px(24.0);
    if let Some(error) = refresh_error {
        pane = pane.child(
            text(format!("Couldn’t refresh review · {error}"))
                .key("refresh-error")
                .role("alert")
                .font(kit.ty.ui)
                .color(kit.c.danger.fg)
                .py(8.0)
                .truncate_middle(),
        );
    }
    let pane = if state.rows.files.is_empty() {
        let message = match state.filter {
            crate::model::Filter::All => "No changes to review",
            crate::model::Filter::Committed => "No committed changes",
            crate::model::Filter::Uncommitted => "No uncommitted changes",
        };
        pane.child(notice(message, None, kit))
    } else {
        let width = cx.layout_size("diff-scroll").map_or(0.0, |size| size[0]);
        pane.child(chrome::legend(
            state,
            kit,
            palette,
            width > 0.0 && width < 565.0,
        ))
        .child(diff::pane(state, kit, palette, width))
    };
    row()
        .key("workspace")
        .grow(1.0)
        .min_h(0.0)
        .child(chrome::sidebar(state, kit, palette))
        .child(pane)
}

fn notice(title: &str, error: Option<&str>, kit: &Kit) -> El {
    let mut content = col()
        .key("review-status")
        .role(if error.is_some() { "alert" } else { "status" })
        .grow(1.0)
        .w_full()
        .center()
        .gap(8.0)
        .px(32.0)
        .child(text(title).font(kit.ty.body_medium).color(kit.c.text_base));
    if let Some(error) = error {
        content = content.child(
            text(error)
                .font(kit.ty.ui)
                .color(kit.c.text_muted)
                .max_w(580.0)
                .wrap(),
        );
    }
    content
}
