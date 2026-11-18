use std::{borrow::Cow, ops::Range, rc::Rc};

use diff_stack::ThreeWayChange;

use rune::{
    Color, Cursor, El, GrowthSpec, Live, MotionIntent, ScrollSpec, Shadow, TextStyle, Transition,
    col, el, icon, memo, rich, row, text,
};
use rune_kit::{Kit, syntax::SyntaxTheme};

use crate::{
    model::{Counts, FileBody, FileDiff, Filter, Line, RowKind},
    palette::Palette,
    state::{HEADER_HEIGHT, State},
};

use super::theme::{Provenance, accent_surface, dot, tint};

const LINE_HEIGHT: f32 = 20.0;
const NUMBER_WIDTH: f32 = 57.0;
const MARKER_WIDTH: f32 = 35.0;
const CODE_SIZE: f32 = 12.0;
const CODE_PADDING: f32 = 19.0;

pub(super) fn pane(state: &State, kit: &Kit, palette: Palette, viewport_width: f32) -> El {
    // Keep the width stable while the virtual window moves through shorter rows.
    #[allow(
        clippy::cast_precision_loss,
        reason = "Convert code columns to layout points"
    )]
    let content_width = code_inset(palette.edge_width)
        + CODE_PADDING
        + state.rows.widest_line as f32 * (0.6 * CODE_SIZE);
    let horizontal = !state.wrap && viewport_width > 0.0 && content_width > viewport_width;
    let show_counts = viewport_width == 0.0 || viewport_width >= 450.0;
    let look = Rc::new(DiffLook::new(
        kit,
        palette,
        horizontal.then_some(viewport_width),
        show_counts,
    ));
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
            move |state: &State, index| {
                render
                    .render_row(state, index)
                    .transition(Transition::opacity())
            },
        )
        .virtual_rows_version(state.rows_version)
        .on_visible_rows(State::show_rows);
    let list = if horizontal {
        list.min_w(content_width)
    } else {
        list
    };
    let scroll = col()
        .key("diff-scroll")
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let scroll = if horizontal {
        scroll.scroll_xy()
    } else {
        scroll
    };
    let scroll = scroll.child(list);
    let mut pane = col().key("diff").grow(1.0).min_h(0.0).child(scroll);
    // A file header can be far outside the virtual window while its file is still in view.
    if let Some(index) = state.sticky_file
        && let Some(file) = state.review.files.get(index)
    {
        pane = pane.child(
            file_header(index, file, state.filter, false, true, show_counts, kit)
                .key(format!("sticky-header/{index}"))
                .absolute(0.0, 0.0)
                .w_full(),
        );
    }
    pane.children(super::pulse::hint(state, kit, palette))
}

fn code_inset(edge_width: f32) -> f32 {
    edge_width + NUMBER_WIDTH * 2.0 + MARKER_WIDTH
}

struct DiffLook {
    kit: Kit,
    palette: Palette,
    syntax: SyntaxTheme,
    code: TextStyle,
    folding: Color,
    horizontal: Option<f32>,
    show_counts: bool,
}

impl DiffLook {
    fn new(kit: &Kit, palette: Palette, horizontal: Option<f32>, show_counts: bool) -> Self {
        Self {
            kit: kit.clone(),
            palette,
            syntax: SyntaxTheme::from_kit(kit),
            code: TextStyle {
                size: CODE_SIZE,
                line_height: LINE_HEIGHT,
                ..kit.ty.mono
            },
            horizontal,
            show_counts,
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
        let filter = state.filter;
        let horizontal = self.horizontal.map(f32::to_bits);
        let version = (
            state.review_version,
            filter,
            state.wrap,
            self.kit.dark,
            self.palette.memo_key(),
            collapsed,
        );
        match &entry.kind {
            RowKind::Line(number) => {
                if let FileBody::Text(diff) = file.body(filter)
                    && let Some(line) = diff.lines.get(*number)
                    && let Some(change) = filter.change(line)
                {
                    let pulse = state
                        .pulse
                        .contains(filter, entry.file, line.worktree)
                        .then(|| state.pulse.opacity());
                    memo(
                        &entry.key,
                        (version, horizontal.is_some(), pulse.is_some()),
                        || self.code_line(line, change, filter, state.wrap, pulse),
                    )
                } else {
                    el().h(LINE_HEIGHT)
                }
            }
            kind => memo(&entry.key, (version, horizontal, self.show_counts), || {
                let row = match kind {
                    RowKind::Header => file_header(
                        entry.file,
                        file,
                        filter,
                        collapsed,
                        false,
                        self.show_counts,
                        &self.kit,
                    ),
                    RowKind::Hunk { label, .. } => self.hunk(label, filter),
                    RowKind::Fold { range, count } => self.fold(entry.file, range.clone(), *count),
                    RowKind::Newline(version) => self.newline(version),
                    RowKind::Placeholder(message) => self.placeholder(message),
                    RowKind::Space => self.space(collapsed),
                    RowKind::Line(_) => el().h(LINE_HEIGHT),
                };
                if let Some(width) = self.horizontal {
                    row.w(width).sticky_left(0.0)
                } else {
                    row
                }
            }),
        }
    }

    fn code_line(
        &self,
        line: &Line,
        change: ThreeWayChange,
        filter: Filter,
        wrap: bool,
        pulse: Option<&Live<f32>>,
    ) -> El {
        let c = &self.kit.c;
        let p = Provenance::new(change, &self.kit, self.palette);
        let (old, new) = filter.columns();
        let head_only = change == ThreeWayChange::CommittedThenRemoved;
        let new_number = if head_only {
            line.head.map_or_else(String::new, |n| format!("H{n}"))
        } else {
            line.number(new).map_or_else(String::new, |n| n.to_string())
        };
        let highlights = line.highlights(filter);
        let code = if highlights.is_empty() {
            text(&line.text)
        } else {
            rich(&line.text, self.syntax.spans(highlights, 0))
        };
        let code = code.font(self.code).color(c.text_base).w_full();
        let code = if wrap {
            code.wrap_anywhere()
        } else {
            code.single_line()
        };
        let code = if let Some(strike) = p.strike {
            code.strike(0, line.text.len(), Some(strike))
        } else {
            code
        };
        let wash =
            || pulse.map(|opacity| super::pulse::wash(&self.kit, self.palette, opacity, 0.0));
        let mut gutter = row()
            .key("gutter")
            .items_stretch()
            .fill(p.gutter)
            .children(wash())
            .child(
                el().key("provenance-edge")
                    .w(self.palette.edge_width)
                    .fill(if p.working {
                        self.palette.edge()
                    } else {
                        Color::TRANSPARENT
                    }),
            )
            .child(
                self.number(
                    line.number(old).map_or_else(String::new, |n| n.to_string()),
                    &p,
                    false,
                )
                .key("old-number"),
            )
            .child(self.number(new_number, &p, head_only).key("new-number"));
        let marker = row().key("mark").w(MARKER_WIDTH).instant_changes();
        let marker = if p.marker.is_empty() {
            marker
        } else {
            marker.child(
                row()
                    .h(LINE_HEIGHT)
                    .gap(5.0)
                    .pl(3.0)
                    .items_center()
                    .motion_intent(MotionIntent::STEP | MotionIntent::CUT)
                    .child(if p.working {
                        dot(self.palette.dot(), 5.0)
                    } else {
                        el().size(5.0, 5.0)
                    })
                    .child(text(p.marker).font(self.code).color(p.ink)),
            )
        };
        gutter = gutter.child(marker);
        if self.horizontal.is_some() {
            gutter = gutter.sticky_left(0.0);
        }
        let line_row = self
            .card_row()
            .min_h(LINE_HEIGHT)
            .items_stretch()
            .fill(p.line)
            .child(gutter)
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .pl(9.0)
                    .pr(10.0)
                    .children(wash())
                    .child(code.key("code")),
            );
        if wrap {
            line_row
        } else {
            line_row.h(LINE_HEIGHT)
        }
    }

    fn number(&self, number: String, p: &Provenance, head_only: bool) -> El {
        let c = &self.kit.c;
        // Keep the column mounted as a number becomes a badge or goes blank.
        // Only its contents cut over; the enclosing row still rides the reflow.
        let cell = row()
            .w(NUMBER_WIDTH)
            .pr(8.0)
            .items_start()
            .justify_end()
            .border_sides([0.0, 1.0, 0.0, 0.0], c.border_muted)
            .instant_changes();
        if number.is_empty() {
            cell
        } else if head_only {
            cell.child(
                text(number)
                    .font(TextStyle {
                        size: 11.0,
                        ..self.code
                    })
                    .color(c.info.fg)
                    .px(5.0)
                    .radius(3.0)
                    .fill(accent_surface(&self.kit))
                    .help("Line number at HEAD")
                    .motion_intent(MotionIntent::STEP | MotionIntent::CUT),
            )
        } else {
            cell.child(
                text(number)
                    .font(self.code)
                    .color(p.ink)
                    .motion_intent(MotionIntent::STEP | MotionIntent::CUT),
            )
        }
    }

    fn hunk(&self, label: &str, filter: Filter) -> El {
        let c = &self.kit.c;
        let note = |name| {
            row()
                .w(NUMBER_WIDTH)
                .pr(8.0)
                .items_center()
                .justify_end()
                .instant_changes()
                .child(
                    text(name)
                        .font(self.kit.ty.small_medium)
                        .motion_intent(MotionIntent::STEP | MotionIntent::CUT)
                        .color(c.text_muted),
                )
        };
        let (old, new) = filter.column_labels();
        self.card_row()
            .h(28.0)
            .items_center()
            .fill(self.folding)
            .child(el().w(self.palette.edge_width))
            .child(note(old))
            .child(note(new))
            .child(el().w(MARKER_WIDTH + 9.0))
            .child(
                text(label)
                    .font(self.kit.ty.mono)
                    .color(c.info.fg)
                    .motion_intent(MotionIntent::STEP | MotionIntent::CUT),
            )
    }

    fn fold(&self, file: usize, range: Range<usize>, hidden: usize) -> El {
        let kit = &self.kit;
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
            .pl(code_inset(self.palette.edge_width) + 6.0)
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
            .pl(code_inset(self.palette.edge_width) + 9.0)
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

fn file_header(
    index: usize,
    file: &FileDiff,
    filter: Filter,
    collapsed: bool,
    sticky: bool,
    show_counts: bool,
    kit: &Kit,
) -> El {
    let c = &kit.c;
    let path = file.path_label(filter);
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
                .motion_intent(MotionIntent::STEP | MotionIntent::CUT)
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
        .on_press_tx(move |state: &mut State, tx| state.toggle_file(index, tx));
    let mut summaries = row()
        .key("counts")
        .items_center()
        .gap(9.0)
        .motion_intent(MotionIntent::STEP | MotionIntent::CUT)
        .instant_changes();
    let mut has_summary = false;
    for layer in [Filter::Committed, Filter::Uncommitted] {
        if show_counts
            && (filter == Filter::All || filter == layer)
            && let Some((counts, label)) = file.summary(layer)
        {
            if has_summary {
                summaries = summaries.child(el().size(1.0, 15.0).fill(c.border_muted));
            }
            summaries = summaries.child(layer_summary(layer, counts, label.as_deref(), kit));
            has_summary = true;
        }
    }
    let header = header.children(has_summary.then_some(summaries));
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

fn layer_summary(layer: Filter, counts: &Counts, label: Option<&str>, kit: &Kit) -> El {
    let c = &kit.c;
    let mut summary = row().items_center().gap(6.0);
    if layer == Filter::Uncommitted {
        summary = summary.child(dot(kit.c.info.icon, 4.0));
    }
    summary = summary.child(
        text(layer.label())
            .font(kit.ty.text_xsmall)
            .color(c.text_muted)
            .single_line(),
    );
    if let Some(label) = label {
        return summary.child(
            text(label)
                .font(kit.ty.text_xsmall_medium)
                .color(c.text_base)
                .single_line(),
        );
    }
    for (value, mark, ink) in [
        (counts.added, "+", c.success.fg),
        (counts.removed, "−", c.danger.fg),
    ] {
        if value > 0 {
            summary = summary.child(text(format!("{mark}{value}")).font(kit.ty.mono).color(ink));
        }
    }
    summary
}
