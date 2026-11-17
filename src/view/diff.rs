use std::{borrow::Cow, ops::Range, rc::Rc};

use rune::{
    Color, Cursor, El, GrowthSpec, ScrollSpec, Shadow, TextStyle, col, el, icon, rich, row, text,
};
use rune_kit::{Kit, syntax::SyntaxTheme};

use crate::{
    model::{Counts, FileBody, FileDiff, Line, RowKind},
    state::{HEADER_HEIGHT, State},
};

use super::theme::{Provenance, accent_surface, dot, tint};

const LINE_HEIGHT: f32 = 23.0;
const NUMBER_WIDTH: f32 = 57.0;
const EDGE_WIDTH: f32 = 3.0;
const MARKER_WIDTH: f32 = 35.0;
const CODE_INSET: f32 = EDGE_WIDTH + NUMBER_WIDTH * 2.0 + MARKER_WIDTH;

pub(super) fn pane(state: &State, kit: &Kit) -> El {
    let look = Rc::new(DiffLook::new(kit));
    let render = Rc::clone(&look);
    let list = col()
        .key("diff-rows")
        .role("list")
        .label("File diffs")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            state.rows.rows.len(),
            LINE_HEIGHT,
            |state: &State, index| {
                state
                    .rows
                    .rows
                    .get(index)
                    .map_or(Cow::Borrowed(""), |row| Cow::Borrowed(row.key.as_str()))
            },
            move |state: &State, index| render.render_row(state, index),
        )
        .virtual_rows_version(state.rows_version)
        .on_visible_rows(State::show_rows);
    let scroll = col()
        .key("diff-scroll")
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list);
    let mut pane = col().key("diff").grow(1.0).min_h(0.0).child(scroll);
    // A file header can be far outside the virtual window while its file is still in view.
    if let Some(index) = state.sticky_file
        && let Some(file) = state.review.files.get(index)
    {
        pane = pane.child(
            file_header(index, file, false, true, kit)
                .key(format!("sticky-header/{index}"))
                .absolute(0.0, 0.0)
                .w_full(),
        );
    }
    pane
}

struct DiffLook {
    kit: Kit,
    syntax: SyntaxTheme,
    code: TextStyle,
    folding: Color,
}

impl DiffLook {
    fn new(kit: &Kit) -> Self {
        Self {
            kit: kit.clone(),
            syntax: SyntaxTheme::from_kit(kit),
            code: TextStyle {
                size: 13.0,
                line_height: LINE_HEIGHT,
                ..kit.ty.mono
            },
            folding: tint(
                kit.c.bg_base,
                kit.c.info.icon,
                if kit.dark { 0.13 } else { 0.08 },
            ),
        }
    }

    fn render_row(&self, state: &State, index: usize) -> El {
        let Some(entry) = state.rows.rows.get(index) else {
            return el();
        };
        let Some(file) = state.review.files.get(entry.file) else {
            return el();
        };
        let collapsed = state.collapsed.contains(&entry.file);
        match &entry.kind {
            RowKind::Header => file_header(entry.file, file, collapsed, false, &self.kit),
            RowKind::Hunk(label) => self.hunk(label),
            RowKind::Line(number) => {
                if let FileBody::Text(diff) = &file.body
                    && let Some(line) = diff.lines.get(*number)
                {
                    self.code_line(line, state.filter.appearance(line.change))
                } else {
                    el().h(LINE_HEIGHT)
                }
            }
            RowKind::Fold(range) => self.fold(entry.file, range.clone()),
            RowKind::Newline(version) => self.newline(version),
            RowKind::Placeholder(message) => self.placeholder(message),
            RowKind::Space => self.space(collapsed),
        }
    }

    fn code_line(&self, line: &Line, change: diff_stack::ThreeWayChange) -> El {
        let c = &self.kit.c;
        let p = Provenance::new(change, &self.kit);
        let head_only = line.base.is_none() && line.worktree.is_none();
        let worktree = if head_only {
            line.head.map_or_else(String::new, |n| format!("H{n}"))
        } else {
            line.worktree.map_or_else(String::new, |n| n.to_string())
        };
        let code = rich(&line.text, self.syntax.spans(&line.highlights, 0))
            .font(self.code)
            .color(c.text_base)
            .grow(1.0)
            .min_w(0.0)
            .single_line()
            .truncate_fade()
            .help_when_truncated(&line.text);
        let code = if p.struck {
            code.strike(0, line.text.len(), None)
        } else {
            code
        };
        let gutter = row()
            .h_full()
            .items_center()
            .fill(p.gutter)
            .child(
                el().w(EDGE_WIDTH)
                    .h_full()
                    .fill(if p.working { c.info.icon } else { p.gutter }),
            )
            .child(self.number(
                line.base.map_or_else(String::new, |n| n.to_string()),
                &p,
                false,
            ))
            .child(self.number(worktree, &p, head_only))
            .child(
                row()
                    .w(MARKER_WIDTH)
                    .h_full()
                    .items_center()
                    .gap(5.0)
                    .pl(3.0)
                    .child(if p.working {
                        dot(&self.kit, 5.0)
                    } else {
                        el().w(5.0)
                    })
                    .child(text(p.marker).font(self.code).color(p.ink)),
            );
        self.card_row()
            .h(LINE_HEIGHT)
            .items_center()
            .fill(p.line)
            .child(gutter)
            .child(code.pl(9.0).pr(10.0))
    }

    fn number(&self, number: String, p: &Provenance, head_only: bool) -> El {
        let c = &self.kit.c;
        let mut digit =
            text(number)
                .font(self.code)
                .color(if head_only { c.info.fg } else { p.ink });
        if head_only {
            digit = digit
                .font(TextStyle {
                    size: 11.0,
                    ..self.code
                })
                .px(5.0)
                .radius(3.0)
                .fill(accent_surface(&self.kit))
                .help("Line number at HEAD");
        }
        row()
            .w(NUMBER_WIDTH)
            .h_full()
            .pr(8.0)
            .items_center()
            .justify_end()
            .border_sides([0.0, 1.0, 0.0, 0.0], c.border_muted)
            .child(digit)
    }

    fn hunk(&self, label: &str) -> El {
        let c = &self.kit.c;
        let note = |name| {
            row()
                .w(NUMBER_WIDTH)
                .pr(8.0)
                .items_center()
                .justify_end()
                .child(
                    text(name)
                        .font(self.kit.ty.small_medium)
                        .color(c.text_muted),
                )
        };
        self.card_row()
            .h(28.0)
            .items_center()
            .fill(self.folding)
            .child(el().w(EDGE_WIDTH))
            .child(note("Base"))
            .child(note("Worktree"))
            .child(el().w(MARKER_WIDTH + 9.0))
            .child(text(label).font(self.kit.ty.mono).color(c.info.fg))
    }

    fn fold(&self, file: usize, range: Range<usize>) -> El {
        let kit = &self.kit;
        let hidden = range.len();
        let label = if hidden == 1 {
            "Expand 1 line".into()
        } else {
            format!("Expand {hidden} lines")
        };
        self.card_row()
            .role("button")
            .label(&label)
            .focusable(true)
            .h(31.0)
            .pl(CODE_INSET + 6.0)
            .items_center()
            .gap(6.0)
            .fill(self.folding)
            .hover(kit.c.hover)
            .press(kit.c.pressed)
            .cursor(Cursor::Pointer)
            .child(icon(kit.glyph.chevron_down, 14.0).color(kit.c.info.fg))
            .child(
                text(label)
                    .font(kit.ty.text_xsmall_medium)
                    .color(kit.c.info.fg),
            )
            .on_press_tx(move |state: &mut State, tx| state.expand(file, range.clone(), tx))
    }

    fn newline(&self, version: &str) -> El {
        let kit = &self.kit;
        self.card_row()
            .h(29.0)
            .items_center()
            .gap(8.0)
            .pl(CODE_INSET + 9.0)
            .fill(kit.c.layer_01)
            .child(text("\\").font(kit.ty.mono).color(kit.c.warning.fg))
            .child(
                text("No newline at end of file")
                    .font(kit.ty.text_xsmall)
                    .color(kit.c.text_base),
            )
            .child(text(version).font(kit.ty.small).color(kit.c.text_muted))
    }

    fn placeholder(&self, message: &str) -> El {
        self.card_row()
            .h(55.0)
            .center()
            .fill(self.kit.c.bg_base)
            .child(
                text(message)
                    .font(self.kit.ty.ui)
                    .color(self.kit.c.text_muted),
            )
    }

    fn space(&self, collapsed: bool) -> El {
        let kit = &self.kit;
        let space = col().w_full().h(19.0);
        if collapsed {
            space
        } else {
            let r = kit.radius.lg;
            space.child(
                el().h(7.0)
                    .w_full()
                    .fill(kit.c.bg_base)
                    .corners([0.0, 0.0, r, r])
                    .border_sides([0.0, 1.0, 1.0, 1.0], kit.c.border_muted),
            )
        }
    }

    fn card_row(&self) -> El {
        row()
            .w_full()
            .border_sides([0.0, 1.0, 0.0, 1.0], self.kit.c.border_muted)
    }
}

fn file_header(index: usize, file: &FileDiff, collapsed: bool, sticky: bool, kit: &Kit) -> El {
    let c = &kit.c;
    let path = file.path_label();
    let chevron = if collapsed {
        kit.glyph.caret_right
    } else {
        kit.glyph.chevron_down
    };
    let r = kit.radius.lg;
    let header = row()
        .role("button")
        .label(if collapsed {
            format!("Expand {path}")
        } else {
            format!("Collapse {path}")
        })
        .expanded(!collapsed)
        .focusable(true)
        .h(HEADER_HEIGHT)
        .px(13.0)
        .gap(9.0)
        .items_center()
        .fill(c.layer_01)
        .corners(if collapsed {
            [r; 4]
        } else if sticky {
            [0.0; 4]
        } else {
            [r, r, 0.0, 0.0]
        })
        .border(1.0, c.border_muted)
        .hover(c.hover)
        .press(c.pressed)
        .cursor(Cursor::Pointer)
        .child(icon(chevron, 16.0).color(c.text_muted))
        .child(
            text(&path)
                .key("path")
                .font(TextStyle {
                    size: 13.0,
                    weight: 550.0,
                    ..kit.ty.mono
                })
                .color(c.text_base)
                .grow(1.0)
                .min_w(0.0)
                .truncate_middle()
                .help_when_truncated(path),
        )
        .child(counts("Committed", &file.committed_counts, false, kit))
        .child(el().size(1.0, 15.0).fill(c.border_muted))
        .child(counts("Uncommitted", &file.uncommitted_counts, true, kit))
        .on_press_tx(move |state: &mut State, tx| state.toggle_file(index, tx));
    if sticky {
        header.box_shadow(Shadow {
            y: 2.0,
            blur: 8.0,
            color: c.scrim,
            ..Shadow::default()
        })
    } else {
        header
    }
}

fn counts(label: &'static str, counts: &Counts, working: bool, kit: &Kit) -> El {
    let c = &kit.c;
    let stat = |value, mark, ink| {
        text(format!("{mark}{value}"))
            .font(kit.ty.mono)
            .color(if value == 0 { c.text_muted } else { ink })
    };
    row()
        .items_center()
        .gap(6.0)
        .child(if working { dot(kit, 4.0) } else { el() })
        .child(
            text(label)
                .font(kit.ty.text_xsmall)
                .color(c.text_muted)
                .single_line(),
        )
        .child(stat(counts.added, "+", c.success.fg))
        .child(stat(counts.removed, "−", c.danger.fg))
}
