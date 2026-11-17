use diff_stack::ThreeWayChange;
use rune::{Cursor, El, ScrollSpec, TextStyle, col, el, row, text};
use rune_kit::Kit;

use crate::{
    input::ChangeKind,
    model::{FileDiff, Filter},
    state::{State, Status},
};

use super::theme::{Provenance, accent_surface, dot};

pub(super) fn toolbar(state: &State, kit: &Kit) -> El {
    let c = &kit.c;
    let title = row()
        .items_center()
        .gap(10.0)
        .child(
            el().size(32.0, 32.0)
                .center()
                .radius(kit.radius.lg)
                .fill(accent_surface(kit))
                .child(
                    text("±")
                        .font(TextStyle {
                            size: 20.0,
                            weight: 600.0,
                            ..kit.ty.mono
                        })
                        .color(c.info.fg),
                ),
        )
        .child(
            text("Review changes")
                .font(TextStyle {
                    size: 17.0,
                    weight: 650.0,
                    ..kit.ty.body
                })
                .color(c.text_base)
                .single_line(),
        );
    let toolbar = row()
        .key("toolbar")
        .h(68.0)
        .px(24.0)
        .items_center()
        .gap(20.0)
        .fill(c.bg_base)
        .border_sides([0.0, 0.0, 1.0, 0.0], c.border_muted)
        .child(title);
    if matches!(&state.status, Status::Ready { .. }) {
        let branches = row()
            .key("comparison")
            .items_center()
            .gap(9.0)
            .min_w(0.0)
            .child(branch("Base", &state.review.base_label, false, kit))
            .child(text("→").font(kit.ty.ui).color(c.text_muted))
            .child(branch("HEAD", &state.review.head_label, true, kit));
        toolbar
            .child(el().size(1.0, 24.0).fill(c.border_muted))
            .child(branches)
            .child(el().grow(1.0))
            .child(filters(state.filter, kit))
    } else {
        toolbar
    }
}

fn branch(label: &'static str, name: &str, head: bool, kit: &Kit) -> El {
    let c = &kit.c;
    row()
        .items_center()
        .gap(7.0)
        .min_w(0.0)
        .child(text(label).font(kit.ty.text_xsmall).color(c.text_muted))
        .child(
            text(name)
                .font(kit.ty.mono)
                .color(if head { c.info.fg } else { c.text_base })
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(name)
                .px(8.0)
                .py(3.0)
                .radius(kit.radius.md)
                .fill(if head {
                    accent_surface(kit)
                } else {
                    c.layer_01
                }),
        )
}

fn filters(current: Filter, kit: &Kit) -> El {
    let c = &kit.c;
    row()
        .key("filter")
        .role("radiogroup")
        .label("Change layers")
        .h(34.0)
        .padding(3.0)
        .gap(2.0)
        .radius(kit.radius.lg)
        .fill(c.layer_01)
        .border(1.0, c.border_muted)
        .children(Filter::ALL.map(|filter| {
            let label = filter.label();
            let selected = current == filter;
            row()
                .key(format!("filter/{}", label.to_ascii_lowercase()))
                .role("radio")
                .label(label)
                .checked(selected)
                .focusable(true)
                .items_center()
                .px(12.0)
                .radius(kit.radius.md)
                .fill(if selected { c.bg_base } else { c.layer_01 })
                .border(f32::from(u8::from(selected)), c.border_muted)
                .hover(c.hover)
                .press(c.pressed)
                .cursor(Cursor::Pointer)
                .child(
                    text(label)
                        .font(kit.ty.text_xsmall_medium)
                        .color(if selected { c.text_base } else { c.text_muted }),
                )
                .on_press_tx(move |state: &mut State, tx| state.set_filter(filter, tx))
        }))
}

pub(super) fn legend(kit: &Kit) -> El {
    use ThreeWayChange::{
        CommittedAdd, CommittedDelete, CommittedThenRemoved, UncommittedAdd, UncommittedDelete,
    };

    let c = &kit.c;
    let marks = |changes: [ThreeWayChange; 2]| {
        row()
            .items_center()
            .gap(3.0)
            .children(changes.into_iter().map(|change| sample(change, kit)))
    };
    row()
        .key("legend")
        .h(56.0)
        .items_center()
        .gap(19.0)
        .child(
            text("Unified diff")
                .font(kit.ty.body_medium)
                .color(c.text_base)
                .single_line(),
        )
        .child(el().grow(1.0))
        .child(legend_item(
            marks([CommittedAdd, CommittedDelete]),
            "Committed",
            kit,
        ))
        .child(legend_item(
            marks([UncommittedAdd, UncommittedDelete]),
            "Uncommitted",
            kit,
        ))
        .child(
            legend_item(sample(CommittedThenRemoved, kit), "Added then removed", kit).help(
                "Added at HEAD and removed in the working tree. H indicates a HEAD line number.",
            ),
        )
}

fn legend_item(mark: El, label: &'static str, kit: &Kit) -> El {
    row().items_center().gap(6.0).child(mark).child(
        text(label)
            .font(kit.ty.text_xsmall)
            .color(kit.c.text_muted)
            .single_line(),
    )
}

fn sample(change: ThreeWayChange, kit: &Kit) -> El {
    let style = Provenance::new(change, kit);
    let label = if style.struck { "Aa" } else { style.marker };
    let mark = text(label).font(kit.ty.mono).color(style.ink);
    let mark = if style.struck {
        mark.strike(0, label.len(), None)
    } else {
        mark
    };
    let swatch = row()
        .h(19.0)
        .px(4.0)
        .gap(3.0)
        .center()
        .radius(kit.radius.sm)
        .fill(style.gutter);
    if style.working {
        swatch.child(dot(kit, 4.0)).child(mark)
    } else {
        swatch.child(mark)
    }
}

pub(super) fn sidebar(state: &State, kit: &Kit) -> El {
    let c = &kit.c;
    let list = col().key("file-list").padding(11.0).gap(4.0).children(
        state
            .review
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| file_item(index, file, state.active_file == Some(index), kit)),
    );
    col()
        .key("sidebar")
        .w(266.0)
        .min_h(0.0)
        .fill(c.bg_base)
        .border_sides([0.0, 1.0, 0.0, 0.0], c.border_muted)
        .child(
            row()
                .h(56.0)
                .px(19.0)
                .items_center()
                .justify_between()
                .border_sides([0.0, 0.0, 1.0, 0.0], c.border_muted)
                .child(
                    text("Files changed")
                        .font(kit.ty.body_medium)
                        .color(c.text_base),
                )
                .child(
                    text(state.review.files.len().to_string())
                        .font(kit.ty.mono)
                        .color(c.text_muted)
                        .px(7.0)
                        .radius(kit.radius.md)
                        .fill(c.layer_01),
                ),
        )
        .child(
            col()
                .key("sidebar-scroll")
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        )
        .child(shortcuts(kit, state.can_refresh()))
}

fn file_item(index: usize, file: &FileDiff, selected: bool, kit: &Kit) -> El {
    let c = &kit.c;
    let path = file.path();
    let (directory, name) = path.rsplit_once('/').unwrap_or(("", &path));
    let mut title = col().grow(1.0).min_w(0.0).gap(2.0).child(
        text(name)
            .font(kit.ty.ui_medium)
            .color(c.text_base)
            .truncate_middle(),
    );
    if !directory.is_empty() {
        title = title.child(
            text(directory)
                .font(kit.ty.small)
                .color(c.text_muted)
                .truncate_middle(),
        );
    }
    let kind = if file.uncommitted == ChangeKind::Unchanged {
        file.committed
    } else {
        file.uncommitted
    };
    let (badge, tint, ink) = match kind {
        ChangeKind::Added => ("A", c.success.bg, c.success.fg),
        ChangeKind::Deleted => ("D", c.danger.bg, c.danger.fg),
        ChangeKind::Renamed => ("R", accent_surface(kit), c.info.fg),
        ChangeKind::Modified => ("M", c.layer_01, c.text_muted),
        ChangeKind::Unchanged => ("", c.bg_base, c.text_muted),
    };
    row()
        .key(format!("file/{index}"))
        .role("button")
        .label(if selected {
            format!("{path}, in view")
        } else {
            path.into_owned()
        })
        .focusable(true)
        .h(46.0)
        .pl(10.0)
        .pr(9.0)
        .gap(9.0)
        .items_center()
        .radius(kit.radius.lg)
        .fill(if selected {
            accent_surface(kit)
        } else {
            c.bg_base
        })
        .border(
            1.0,
            if selected {
                c.info.border
            } else {
                rune::Color::TRANSPARENT
            },
        )
        .hover(c.hover)
        .press(c.pressed)
        .cursor(Cursor::Pointer)
        .child(el().size(3.0, 17.0).radius(1.5).fill(if selected {
            c.info.icon
        } else {
            rune::Color::TRANSPARENT
        }))
        .child(title)
        .child(if file.uncommitted == ChangeKind::Unchanged {
            el().w(5.0)
        } else {
            dot(kit, 5.0)
        })
        .child(
            el().size(22.0, 21.0)
                .center()
                .radius(kit.radius.sm)
                .fill(tint)
                .child(text(badge).font(kit.ty.mono).color(ink)),
        )
        .on_press_tx(move |state: &mut State, tx| state.jump_to_file(index, tx))
}

fn shortcuts(kit: &Kit, can_refresh: bool) -> El {
    let c = &kit.c;
    let shortcut = |keys: &[&str], label: &str| {
        row()
            .items_center()
            .gap(4.0)
            .children(keys.iter().map(|&key| {
                el().size(17.0, 18.0)
                    .center()
                    .radius(kit.radius.sm)
                    .fill(c.layer_01)
                    .border(1.0, c.border_muted)
                    .child(text(key).font(kit.ty.mono).color(c.text_muted))
            }))
            .child(
                text(label)
                    .font(kit.ty.text_xsmall)
                    .color(c.text_muted)
                    .pl(2.0),
            )
    };
    let list = row()
        .key("shortcuts")
        .h(43.0)
        .px(19.0)
        .gap(12.0)
        .items_center()
        .border_sides([1.0, 0.0, 0.0, 0.0], c.border_muted)
        .child(shortcut(&["j", "k"], "Hunks"))
        .child(shortcut(&["[", "]"], "Files"));
    if can_refresh {
        list.child(shortcut(&["r"], "Refresh"))
    } else {
        list
    }
}
