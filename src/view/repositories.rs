use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use rune::{Cursor, El, GrowthSpec, ScrollSpec, Tx, col, el, row, text};
use rune_kit::{Kit, MenuItem, context_menu};

use crate::{
    panels::{Panel, PanelRoom},
    repositories::{Entry, Workspace, row_key},
    state::State,
};

use super::theme::{accent_surface, dot};

const ROW: f32 = 68.0;

pub(super) fn sidebar(
    state: &State,
    workspace: &Workspace,
    room: PanelRoom,
    kit: &Kit,
    layer: &mut Vec<El>,
) -> El {
    let color = &kit.c;
    let compact = workspace.collapsed;
    let header = if compact {
        col().items_center().py(10.0).gap(4.0)
    } else {
        row().h(56.0).px(13.0).items_center().gap(4.0).child(
            text("Repositories")
                .font(kit.ty.body_medium)
                .color(color.text_base)
                .grow(1.0),
        )
    };
    let expand = if compact {
        "Expand repositories"
    } else {
        "Collapse repositories"
    };
    let header = header
        .w_full()
        .border_sides([0.0, 0.0, 1.0, 0.0], color.border_muted)
        .child(square(
            "repositories-toggle",
            if compact { "›" } else { "‹" },
            expand,
            kit,
            State::toggle_repositories,
        ))
        .child(square(
            "add-repository",
            "+",
            "Add repository (⌘O)",
            kit,
            State::pick_folder,
        ));
    let list = repository_list(workspace, kit);
    let footer = col()
        .key("repository-shortcuts")
        .w_full()
        .gap(5.0)
        .px(12.0)
        .py(12.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], color.border_muted)
        .child(hint("⌘1–9", "Jump to repository", kit))
        .child(hint("⌘[ / ⌘]", "Previous / next", kit))
        .child(hint("⌘O", "Add repository", kit));
    let mut panel = col()
        .key("repository-sidebar")
        .role("navigation")
        .label("Repository sidebar")
        .w(state.panels.width(Panel::Repositories, room, compact))
        .min_h(0.0)
        .fill(color.bg_base)
        .border_sides([0.0, 1.0, 0.0, 0.0], color.border_muted)
        .child(header)
        .child(list)
        .children((!compact).then_some(footer));
    if let Some((path, point)) = &workspace.menu {
        let path = path.clone();
        panel =
            panel.child(
                context_menu("repository-context", el().absolute(0.0, 0.0), Some(*point))
                    .item(MenuItem::new("Remove from sidebar").on_select_tx(
                        move |state: &mut State, tx| state.remove_repository(&path, tx),
                    ))
                    .on_close(State::close_menu)
                    .build(kit, layer),
            );
    }
    panel.children((!compact).then(|| super::panels::handle(Panel::Repositories, room, state, kit)))
}

fn repository_list(workspace: &Workspace, kit: &Kit) -> El {
    let style = kit.clone();
    col()
        .key("repository-list")
        .role("list")
        .label("Repositories")
        .grow(1.0)
        .w_full()
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(
            col()
                .key("repository-rows")
                .padding(6.0)
                .w_full()
                .growth(GrowthSpec::default())
                .virtual_rows(
                    workspace.entries.len(),
                    if workspace.collapsed { 43.0 } else { ROW },
                    |state: &State, index| {
                        state
                            .workspace
                            .as_ref()
                            .and_then(|workspace| workspace.entries.get(index))
                            .map_or(Cow::Borrowed(""), |entry| {
                                Cow::Owned(row_key(&entry.repository.path))
                            })
                    },
                    move |state: &State, index| {
                        state
                            .workspace
                            .as_ref()
                            .and_then(|workspace| {
                                workspace
                                    .entries
                                    .get(index)
                                    .map(|entry| entry_row(workspace, entry, index, &style))
                            })
                            .unwrap_or_else(el)
                    },
                ),
        )
}

fn square(
    key: &'static str,
    glyph: &'static str,
    label: &'static str,
    kit: &Kit,
    action: fn(&mut State, &mut Tx),
) -> El {
    el().key(key)
        .role("button")
        .label(label)
        .help(label)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .size(30.0, 30.0)
        .center()
        .radius(kit.radius.md)
        .hover(kit.c.hover)
        .press(kit.c.pressed)
        .child(
            text(glyph)
                .font(rune::TextStyle {
                    size: 21.0,
                    ..kit.ty.ui
                })
                .color(kit.c.text_base),
        )
        .on_press_tx(action)
}

fn entry_row(workspace: &Workspace, entry: &Entry, index: usize, kit: &Kit) -> El {
    let path = entry.repository.path.clone();
    let selected = workspace.selected == Some(index);
    let branch = if entry.summary.branch.is_empty() {
        "Checking…"
    } else {
        &entry.summary.branch
    };
    let status = if entry.summary.dirty {
        ", uncommitted changes"
    } else {
        ""
    };
    let help = format!("{} · {branch}{status}", path.display());
    let item = row()
        .key(row_key(&path))
        .role("button")
        .label(format!("{}, {help}", entry.name))
        .checked(selected)
        .focusable(true)
        .cursor(Cursor::Pointer)
        .w_full()
        .h(if workspace.collapsed { 43.0 } else { ROW })
        .items_center()
        .padding(4.0)
        .on_press_tx({
            let path = path.clone();
            move |state: &mut State, tx| state.select_repository_path(&path, tx)
        })
        .on_secondary_press(move |state: &mut State, point, _| {
            state.edit_menu(path.clone(), point);
        });
    let body = row()
        .w_full()
        .h_full()
        .items_center()
        .gap(9.0)
        .px(if workspace.collapsed { 3.0 } else { 7.0 })
        .radius(kit.radius.md)
        .fill(if selected {
            accent_surface(kit)
        } else {
            kit.c.bg_base
        })
        .hover(kit.c.hover)
        .press(kit.c.pressed);
    let body = body.child(badge(entry, selected, kit));
    let body = if workspace.collapsed {
        body.children(entry.summary.dirty.then(|| dirty(kit).absolute(27.0, 4.0)))
    } else {
        with_labels(body, workspace, entry, branch, kit)
    };
    item.help(help).child(body)
}

fn badge(entry: &Entry, selected: bool, kit: &Kit) -> El {
    let initials: String = entry
        .name
        .chars()
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    el().size(28.0, 28.0)
        .center()
        .radius(kit.radius.md)
        .fill(if selected {
            kit.c.bg_base
        } else {
            kit.c.layer_01
        })
        .child(
            text(initials)
                .font(kit.ty.text_xsmall_medium)
                .color(if selected {
                    kit.c.info.fg
                } else {
                    kit.c.text_muted
                }),
        )
}

fn with_labels(body: El, workspace: &Workspace, entry: &Entry, branch: &str, kit: &Kit) -> El {
    let hint = short_hint(entry, &workspace.entries);
    body.child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .gap(2.0)
            .child(
                text(entry.name.clone())
                    .font(kit.ty.ui_medium)
                    .color(kit.c.text_base)
                    .truncate_middle(),
            )
            .child(
                text(branch)
                    .font(kit.ty.text_xsmall)
                    .color(kit.c.text_muted)
                    .truncate_middle(),
            )
            .children(hint.map(|hint| {
                text(hint)
                    .font(kit.ty.text_xsmall)
                    .color(kit.c.text_muted)
                    .truncate_middle()
            })),
    )
    .children(entry.summary.dirty.then(|| dirty(kit)))
}

fn dirty(kit: &Kit) -> El {
    dot(kit.c.info.icon, 6.0)
        .key("repository-dirty")
        .role("img")
        .label("Uncommitted changes")
}

fn hint(keys: &'static str, caption: &'static str, kit: &Kit) -> El {
    row()
        .items_center()
        .gap(7.0)
        .child(
            text(keys)
                .font(kit.ty.text_xsmall_medium)
                .color(kit.c.text_base)
                .single_line(),
        )
        .child(
            text(caption)
                .font(kit.ty.text_xsmall)
                .color(kit.c.text_muted)
                .single_line(),
        )
}

fn short_hint(entry: &Entry, entries: &[Entry]) -> Option<String> {
    let path = &entry.repository.path;
    let duplicates: Vec<_> = entries
        .iter()
        .filter(|other| other.name == entry.name && other.repository.path != *path)
        .collect();
    if duplicates.is_empty() {
        return None;
    }
    let components: Vec<_> = path.components().collect();
    for length in 2..components.len() {
        let suffix: PathBuf = components.iter().rev().take(length).rev().collect();
        if duplicates
            .iter()
            .all(|other| !other.repository.path.ends_with(&suffix))
        {
            return Some(format!(
                "…/{}",
                suffix.parent().unwrap_or(Path::new("")).display()
            ));
        }
    }
    Some(path.parent().unwrap_or(path).display().to_string())
}
