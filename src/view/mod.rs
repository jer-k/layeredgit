//! Native review window and its virtualized diff pane.

mod chrome;
mod diff;
mod panels;
mod pulse;
mod repositories;
mod revisions;
mod theme;

use rune::{Cx, El, col, row, text};
use rune_kit::{Button, ButtonVariant, Kit};

use crate::{
    palette::Palette,
    panels::{MIN_DIFF_WIDTH, MIN_WINDOW_WIDTH, Panel, PanelRoom},
    state::{State, Status},
};

/// Height of the review toolbar and native controls.
pub(crate) const TOOLBAR_HEIGHT: f32 = 68.0;
const DIFF_PADDING: f32 = 24.0;

/// Draws the application in the current system appearance.
pub(crate) fn review(state: &State, cx: &mut Cx<State>) -> El {
    if state.headless_reduced_motion {
        cx.reduced_motion(true);
    }
    let kit = Kit::neutral(cx.env().system_dark);
    cx.caption_color(kit.c.text_base);
    let palette = state.palette(kit.dark);
    let mut layer = Vec::new();
    let width = cx
        .layout_with("review", |rect| rect[2])
        .unwrap_or(MIN_WINDOW_WIDTH);
    let collapsed = state
        .workspace
        .as_ref()
        .map(|workspace| workspace.collapsed);
    let repositories = PanelRoom::new(Panel::Repositories, width, collapsed);
    let files = PanelRoom::new(Panel::Files, width, collapsed);
    let content = match &state.status {
        Status::Empty => empty(state, &kit),
        Status::Loading | Status::Changing => notice("Loading review…", None, &kit),
        Status::Failed(error) => notice("Couldn’t load review", Some(error), &kit),
        Status::Ready { refresh_error } => {
            let diff_width = if cfg!(feature = "dev") {
                // The palette tuner can dock next to the diff in dev builds.
                cx.layout_with("diff-scroll", |rect| rect[2]).unwrap_or(0.0)
            } else {
                let repo_width = collapsed.map_or(0.0, |collapsed| {
                    state
                        .panels
                        .width(Panel::Repositories, repositories, collapsed)
                });
                width
                    - repo_width
                    - state.panels.width(Panel::Files, files, false)
                    - 2.0 * DIFF_PADDING
            };
            workspace(
                state,
                refresh_error.as_deref(),
                &kit,
                palette,
                files,
                diff_width,
            )
        }
    };
    #[cfg(feature = "dev")]
    let content = state.tuning.panel(content, kit.dark);
    let toolbar = chrome::toolbar(state, &kit, cx.chrome_insets(), &mut layer);
    let body = if let Some(workspace) = &state.workspace {
        let content = col()
            .grow(1.0)
            .min_w(0.0)
            .min_h(0.0)
            .children(
                workspace
                    .add_error
                    .as_ref()
                    .map(|error| alert("open-error", error, &kit)),
            )
            .children(
                workspace
                    .active()
                    .and_then(|entry| entry.revision_error.as_deref())
                    .map(|error| alert("revision-error", error, &kit)),
            )
            .child(content);
        row()
            .key("repository-workspace")
            .grow(1.0)
            .min_h(0.0)
            .child(repositories::sidebar(
                state,
                workspace,
                repositories,
                &kit,
                &mut layer,
            ))
            .child(content)
    } else {
        content
    };
    let body = col()
        .key("review")
        .w_full()
        .h_full()
        .fill(kit.c.page)
        .child(toolbar)
        .child(body);
    rune_kit::root(body, layer)
}

fn alert(key: &'static str, error: &str, kit: &Kit) -> El {
    text(error)
        .key(key)
        .role("alert")
        .font(kit.ty.ui)
        .color(kit.c.danger.fg)
        .px(18.0)
        .py(9.0)
        .w_full()
        .fill(kit.c.danger.bg)
        .wrap()
}

fn empty(state: &State, kit: &Kit) -> El {
    let pending = state
        .workspace
        .as_ref()
        .is_some_and(|workspace| workspace.pending);
    col()
        .key("empty-workspace")
        .grow(1.0)
        .min_h(0.0)
        .w_full()
        .center()
        .gap(12.0)
        .px(32.0)
        .child(chrome::review_mark(kit, 48.0))
        .child(
            text("Review a repository")
                .role("heading")
                .font(kit.ty.body_medium)
                .color(kit.c.text_base),
        )
        .child(
            text("Add a working tree to review committed and uncommitted changes together.")
                .font(kit.ty.ui)
                .color(kit.c.text_muted)
                .wrap(),
        )
        .child(
            Button::new(if pending {
                "Opening…"
            } else {
                "Add repository…"
            })
            .variant(ButtonVariant::Contrast)
            .disabled(pending)
            .on_press_tx(State::pick_folder)
            .build(kit)
            .key("empty-add-repository"),
        )
}

fn workspace(
    state: &State,
    refresh_error: Option<&str>,
    kit: &Kit,
    palette: Palette,
    files: PanelRoom,
    diff_width: f32,
) -> El {
    let mut pane = col()
        .key("review-pane")
        .grow(1.0)
        .min_w(MIN_DIFF_WIDTH)
        .min_h(0.0)
        .px(DIFF_PADDING);
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
        pane.child(chrome::legend(
            state,
            kit,
            palette,
            diff_width > 0.0 && diff_width < 565.0,
        ))
        .child(diff::pane(state, kit, palette, diff_width))
    };
    row()
        .key("workspace")
        .grow(1.0)
        .min_h(0.0)
        .child(chrome::sidebar(state, kit, palette, files))
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
