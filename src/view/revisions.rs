use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use rune::{Cursor, El, icon, row, text};
use rune_kit::{Combobox, ComboboxOption, Kit};

use crate::{
    desktop::path_key,
    repositories::{Entry, Side},
    source::ref_label,
    state::State,
};

use super::theme::accent_surface;

pub(super) fn render(state: &State, side: Side, kit: &Kit, layer: &mut Vec<El>) -> El {
    let name = state.active_label(side);
    let key = match side {
        Side::Base => "base-picker",
        Side::Head => "head-picker",
    };
    let trigger = trigger(key, &name, side, kit);
    let Some(workspace) = &state.workspace else {
        return trigger;
    };
    let Some(entry) = workspace.active() else {
        return trigger;
    };
    let Some(combo) = workspace.picker.get(side.index()) else {
        return trigger;
    };
    let (options, values) = options(entry, side, combo.search().text.trim());
    let default = match side {
        Side::Base => entry.repository.base.is_none(),
        Side::Head => entry.head.is_none(),
    };
    let current = current(entry, side, &name).or_else(|| default.then(|| "reset".into()));
    Combobox::new(key, combo, &options, move |state: &mut State| {
        state.picker_mut(side)
    })
    .label(format!("{}: {name}", side.label()))
    .current(current.as_deref())
    .search_placeholder("Search branches or enter a revision")
    .width(323.0)
    .max_height(300.0)
    .trigger(trigger)
    .on_choose(move |state: &mut State, id, tx| {
        if let Some((_, revision)) = values.iter().find(|(key, _)| key == id) {
            state.choose_revision(side, revision.clone(), tx);
        }
    })
    .build(kit, layer)
    // The popover's trigger wrapper sizes to its content; let it shrink so long
    // names truncate inside the chip instead of running under the filters.
    .shrink()
    .min_w(0.0)
}

fn trigger(key: &'static str, name: &str, side: Side, kit: &Kit) -> El {
    let head = side == Side::Head;
    row()
        .key(key)
        .role("button")
        .label(format!("{}: {name}", side.label()))
        .focusable(true)
        .cursor(Cursor::Pointer)
        .items_center()
        .gap(7.0)
        .shrink()
        .min_w(0.0)
        .child(
            text(side.label())
                .font(kit.ty.text_xsmall_medium)
                .color(kit.c.text_muted)
                .single_line(),
        )
        .child(
            row()
                .items_center()
                .gap(6.0)
                .px(8.0)
                .h(30.0)
                .min_w(0.0)
                .max_w(190.0)
                .shrink()
                .radius(kit.radius.md)
                .fill(if head {
                    accent_surface(kit)
                } else {
                    kit.c.layer_01
                })
                .border(
                    1.0,
                    if head {
                        kit.c.info.border
                    } else {
                        kit.c.border_muted
                    },
                )
                .hover(kit.c.hover)
                .child(
                    text(name)
                        .font(kit.ty.mono)
                        .color(if head { kit.c.info.fg } else { kit.c.text_base })
                        .shrink()
                        .min_w(0.0)
                        .truncate_middle()
                        .help_when_truncated(name),
                )
                .child(icon(kit.glyph.chevron_down, 14.0).color(kit.c.text_muted)),
        )
}

type ChoiceValues = Vec<(String, Option<OsString>)>;

fn options(entry: &Entry, side: Side, query: &str) -> (Vec<ComboboxOption>, ChoiceValues) {
    let reset = match side {
        Side::Base => "Use default base",
        Side::Head => "Reset to current",
    };
    let mut options = vec![ComboboxOption::new("reset", reset)];
    let mut values = vec![("reset".into(), None)];
    let mut group = String::new();
    let mut exact = false;
    for branch in &entry.branches {
        let name = branch.name.to_string_lossy();
        let next = if branch.is_remote {
            name.split_once('/')
                .map_or("Remote branches", |(remote, _)| remote)
        } else {
            "Local branches"
        };
        if next != group {
            next.clone_into(&mut group);
            if query.is_empty() {
                let caption = if branch.is_remote {
                    format!("Remote · {group}")
                } else {
                    group.clone()
                };
                let mut heading = ComboboxOption::new(format!("group-{}", options.len()), caption);
                heading.disabled = true;
                options.push(heading);
            }
        }
        let key = branch_key(&branch.reference);
        let mut item = ComboboxOption::new(key.clone(), name.as_ref());
        exact |= query == name;
        item.detail = if branch.is_current {
            "Current checkout".into()
        } else if let Some(upstream) = &branch.upstream {
            format!("Tracks {}", ref_label(upstream))
        } else {
            branch.tip.chars().take(7).collect()
        };
        values.push((key, Some(branch.reference.clone())));
        options.push(item);
    }
    if !query.is_empty() && !exact {
        let mut custom = ComboboxOption::new("revision", query);
        custom.detail = "Use as revision · tag, SHA, or expression".into();
        options.insert(0, custom);
        values.push(("revision".into(), Some(query.into())));
    }
    (options, values)
}

fn current(entry: &Entry, side: Side, name: &str) -> Option<String> {
    let reference = match side {
        Side::Base => entry.repository.base.as_ref(),
        Side::Head => entry.head.as_ref(),
    };
    let reference = if side == Side::Head && reference.is_none() {
        entry.branches.iter().find(|branch| branch.is_current)
    } else {
        entry.branches.iter().find(|branch| {
            reference.map_or(branch.name == name, |chosen| {
                *chosen == branch.reference || *chosen == branch.name
            })
        })
    }?;
    Some(branch_key(&reference.reference))
}

fn branch_key(reference: &OsStr) -> String {
    format!("ref-{}", path_key(Path::new(reference)))
}
