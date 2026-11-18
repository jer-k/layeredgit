use diff_stack::ThreeWayChange;
use rune::{ChromeInsets, Cursor, El, TextStyle, col, el, row, text};
use rune_kit::{FileNode, Kit, Tree, TreeItem, TreeStyle};

use crate::{
    input::ChangeKind,
    model::Filter,
    state::{State, Status},
};

use super::theme::{Provenance, accent_surface, dot};

pub(super) fn toolbar(state: &State, kit: &Kit, insets: ChromeInsets) -> El {
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
        .h(super::TOOLBAR_HEIGHT)
        .pl(insets.leading + 24.0)
        .pr(insets.trailing + 24.0)
        .items_center()
        .gap(20.0)
        .fill(c.bg_base)
        .window_drag()
        .border_sides([0.0, 0.0, 1.0, 0.0], c.border_muted)
        .child(title);
    if matches!(&state.status, Status::Ready { .. }) {
        let branches = row()
            .key("comparison")
            .items_center()
            .gap(9.0)
            .shrink()
            .min_w(0.0)
            .child(branch("Base", &state.review.base_label, false, kit))
            .child(text("→").font(kit.ty.ui).color(c.text_muted))
            .child(branch("HEAD", &state.review.head_label, true, kit));
        toolbar
            .child(el().size(1.0, 24.0).fill(c.border_muted))
            .child(branches)
            .child(el().grow(1.0).h_full().window_drag())
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
        .shrink()
        .min_w(0.0)
        .child(text(label).font(kit.ty.text_xsmall).color(c.text_muted))
        .child(
            text(name)
                .font(kit.ty.mono)
                .color(if head { c.info.fg } else { c.text_base })
                .shrink()
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

pub(super) fn legend(state: &State, kit: &Kit) -> El {
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
        .gap(12.0)
        .child(
            text("Unified diff")
                .font(kit.ty.body_medium)
                .color(c.text_base)
                .single_line(),
        )
        .child(el().grow(1.0))
        .children([
            (state.filter != Filter::Uncommitted).then(|| legend_item(
                marks([CommittedAdd, CommittedDelete]), "Committed", kit,
            )),
            (state.filter != Filter::Committed).then(|| legend_item(
                marks([UncommittedAdd, UncommittedDelete]), "Uncommitted", kit,
            )),
            (state.filter == Filter::All).then(|| {
                legend_item(sample(CommittedThenRemoved, kit), "Added then removed", kit).help(
                    "Added at HEAD and removed in the working tree. H indicates a HEAD line number.",
                )
            }),
        ].into_iter().flatten())
        .child(wrap_toggle(state.wrap, kit))
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

fn wrap_toggle(wrap: bool, kit: &Kit) -> El {
    row()
        .key("wrap")
        .role("checkbox")
        .label("Wrap lines")
        .checked(wrap)
        .focusable(true)
        .h(30.0)
        .px(9.0)
        .items_center()
        .radius(kit.radius.md)
        .fill(if wrap {
            accent_surface(kit)
        } else {
            kit.c.bg_base
        })
        .border(
            1.0,
            if wrap {
                kit.c.info.border
            } else {
                kit.c.border_muted
            },
        )
        .hover(kit.c.hover)
        .press(kit.c.pressed)
        .cursor(Cursor::Pointer)
        .child(
            text("Wrap")
                .font(kit.ty.text_xsmall_medium)
                .color(if wrap { kit.c.info.fg } else { kit.c.text_base })
                .single_line(),
        )
        .on_press_tx(State::toggle_wrap)
}

pub(super) fn sidebar(state: &State, kit: &Kit) -> El {
    let c = &kit.c;
    let mut style = TreeStyle::from_kit(kit).gap(7.0).radius(kit.radius.md);
    style.row_height = 32.0;
    style.indent = 15.0;
    style.padding = 9.0;
    style.selected = accent_surface(kit);
    let row_kit = kit.clone();
    let list = Tree::new(State::sidebar, State::sidebar_mut, FileNode::label)
        .name("Changed files")
        .style(style)
        .row(move |node, row| {
            let item = TreeItem::file(node, row);
            let Some(badge) = &node.data else { return item };
            let c = &row_kit.c;
            let (glyph, surface, ink) = match badge.kind {
                ChangeKind::Added => ("A", c.success.bg, c.success.fg),
                ChangeKind::Deleted => ("D", c.danger.bg, c.danger.fg),
                ChangeKind::Renamed => ("R", accent_surface(&row_kit), c.info.fg),
                _ => ("M", c.layer_01, c.text_muted),
            };
            let item = if badge.working {
                item.dot(c.info.icon)
            } else {
                item
            };
            item.trailing(
                el().size(22.0, 21.0)
                    .center()
                    .radius(row_kit.radius.sm)
                    .fill(surface)
                    .child(text(glyph).font(row_kit.ty.mono).color(ink)),
            )
        })
        .on_select(State::sidebar_select)
        .build(state, kit)
        .on_key(State::sidebar_key)
        .grow(1.0)
        .min_h(0.0)
        .padding(9.0);
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
                    text(state.rows.files.len().to_string())
                        .font(kit.ty.mono)
                        .color(c.text_muted)
                        .px(7.0)
                        .radius(kit.radius.md)
                        .fill(c.layer_01),
                ),
        )
        .child(list)
        .child(shortcuts(kit, state.can_refresh()))
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
