//! Repository picker, recent working trees, and inline launch errors.

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use rune::{
    App, AppSpec, Cursor, Cx, El, FileDialogOptions, KeyEvent, Titlebar, Tx, col, el, row, text,
};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::{
    APP_NAME,
    desktop::{Client, open_shortcut, path_key, repo_name},
    source::Repository,
    view,
};

const TITLEBAR_HEIGHT: f32 = 48.0;

#[derive(Default)]
pub(crate) struct Start {
    client: Option<Client>,
    recent: Vec<PathBuf>,
    error: Option<String>,
    pending: bool,
}

impl Start {
    pub(crate) fn live(client: Client) -> Self {
        Self {
            client: Some(client),
            ..Self::default()
        }
    }

    pub(crate) fn preview(recent: Vec<PathBuf>) -> Self {
        Self {
            recent,
            ..Self::default()
        }
    }

    pub(crate) fn set_recents(&mut self, paths: Vec<PathBuf>) {
        self.recent = paths;
    }

    pub(crate) fn show(&self) {
        if let Some(client) = &self.client {
            client.show_start();
        }
    }

    pub(crate) fn resolved(
        &mut self,
        result: Result<(Repository, String), String>,
        warning: Option<String>,
        tx: &mut Tx,
    ) {
        self.pending = false;
        match result {
            Ok((repository, head)) => {
                self.error = warning;
                if let Some(client) = &self.client {
                    client.show_review(repository, head);
                }
            }
            Err(error) => {
                self.error = Some(error);
                self.show();
            }
        }
        tx.instant();
    }

    fn pick(&mut self, tx: &mut Tx) {
        if self.pending {
            return;
        }
        self.error = None;
        let options = FileDialogOptions {
            title: "Open repository".into(),
            ..FileDialogOptions::default()
        };
        tx.open_folder(options, |state: &mut Self, answer, _| match answer {
            Ok(Some(path)) => state.open(path),
            Ok(None) => {}
            Err(error) => state.error = Some(error.to_string()),
        });
    }

    fn open(&mut self, path: PathBuf) {
        if let Some(client) = &self.client {
            self.pending = client.open(path);
            self.error = (!self.pending).then(|| "Couldn’t reach the repository worker.".into());
        }
    }

    fn key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if !self.pending && open_shortcut(event) {
            self.pick(tx);
            true
        } else {
            false
        }
    }
}

pub(crate) fn app(size: (f32, f32)) -> App<Start> {
    App::new(AppSpec {
        min_size: Some((460.0, 300.0)),
        on_key: Some(Start::key),
        ..AppSpec::new(APP_NAME, size, Start::default, render)
    })
    .titlebar(Titlebar::new(TITLEBAR_HEIGHT))
    .tab_visits_controls(true)
}

fn render(state: &Start, cx: &mut Cx<Start>) -> El {
    let kit = Kit::neutral(cx.env().system_dark);
    let c = &kit.c;
    cx.caption_color(c.text_base);
    let header = row()
        .w_full()
        .items_center()
        .gap(12.0)
        .child(view::review_mark(&kit, 42.0))
        .child(
            text(APP_NAME)
                .font(rune::TextStyle {
                    size: 24.0,
                    weight: 650.0,
                    ..kit.ty.body
                })
                .color(c.text_base),
        )
        .child(el().grow(1.0))
        .child(
            Button::new(if state.pending {
                "Opening…"
            } else {
                "Open repository…"
            })
            .variant(ButtonVariant::Contrast)
            .control_size(ControlSize::Large)
            .shortcut("⌘O")
            .disabled(state.pending)
            .on_press_tx(Start::pick)
            .build(&kit)
            .key("open-repository"),
        );
    let mut body = col()
        .grow(1.0)
        .min_h(0.0)
        .px(28.0)
        .pb(24.0)
        .gap(16.0)
        .child(header);
    if let Some(error) = &state.error {
        body = body.child(
            text(error)
                .key("open-error")
                .role("alert")
                .font(kit.ty.ui)
                .color(c.danger.fg)
                .wrap(),
        );
    }
    col()
        .key("start")
        .w_full()
        .h_full()
        .fill(c.page)
        .child(row().h(TITLEBAR_HEIGHT).w_full().window_drag())
        .child(body.child(recents(state, &kit)))
}

fn recents(state: &Start, kit: &Kit) -> El {
    let c = &kit.c;
    let list = col()
        .key("recent-repositories")
        .grow(1.0)
        .min_h(0.0)
        .radius(kit.radius.lg)
        .fill(c.bg_base)
        .border(1.0, c.border_muted);
    let list = if state.recent.is_empty() {
        list.center().child(
            text("Repositories you open will appear here.")
                .font(kit.ty.ui)
                .color(c.text_muted),
        )
    } else {
        let kit = kit.clone();
        list.scroll(rune::ScrollSpec {
            stick_bottom: false,
        })
        .child(
            col()
                .key("recent-rows")
                .w_full()
                .px(5.0)
                .py(5.0)
                .growth(rune::GrowthSpec::default())
                .virtual_rows(
                    state.recent.len(),
                    50.0,
                    |state: &Start, index| {
                        state
                            .recent
                            .get(index)
                            .map_or(Cow::Borrowed(""), |path| Cow::Owned(recent_key(path)))
                    },
                    move |state: &Start, index| {
                        state
                            .recent
                            .get(index)
                            .map_or_else(el, |path| recent_row(path, state.pending, &kit))
                    },
                ),
        )
    };
    col()
        .grow(1.0)
        .min_h(0.0)
        .gap(8.0)
        .child(
            text("Recent repositories")
                .font(kit.ty.body_medium)
                .color(c.text_base)
                .role("heading"),
        )
        .child(list)
}

fn recent_row(path: &Path, pending: bool, kit: &Kit) -> El {
    let name = repo_name(path);
    let location = path.to_string_lossy().into_owned();
    let key = recent_key(path);
    let path = path.to_owned();
    let item = row()
        .key(key)
        .role("button")
        .label(format!("{name}, {location}"))
        .focusable(!pending)
        .w_full()
        .items_center()
        .radius(kit.radius.md)
        .px(12.0)
        .py(8.0)
        .child(
            col()
                .gap(3.0)
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(name)
                        .font(kit.ty.body_medium)
                        .color(kit.c.text_base)
                        .truncate_middle(),
                )
                .child(
                    text(location.clone())
                        .font(kit.ty.text_xsmall)
                        .color(kit.c.text_muted)
                        .truncate_middle()
                        .help_when_truncated(location),
                ),
        );
    if pending {
        item
    } else {
        item.cursor(Cursor::Pointer)
            .hover(kit.c.hover)
            .press(kit.c.pressed)
            .on_press(move |state: &mut Start| state.open(path.clone()))
    }
}

fn recent_key(path: &Path) -> String {
    format!("recent:{}", path_key(path))
}
