//! Text selection across a virtualized diff. The scroll pane owns the gesture, so Rune
//! keeps delivering it even after the code row that started it has unmounted.

mod paint;
#[cfg(test)]
mod tests;

use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    rc::{Rc, Weak},
    time::Duration,
};

use rune::{
    Color, Cursor, Drag, El, Key, KeyEvent, Live, Phase, PopupItem, PopupMenu, Span, TextStyle, Tx,
    col,
};

use crate::{
    model::{
        FileBody, Line, RowKind,
        selection::{Point, Selection},
    },
    state::{HEADER_HEIGHT, State},
};

use paint::LinePaint;

pub(crate) const SCROLL_KEY: &str = "diff-scroll";
const REPEAT: Duration = Duration::from_micros(16_667);

type Row = (usize, usize);

/// The code text's layout parameters, shared with its selection highlight.
#[derive(Clone, Copy)]
pub(crate) struct CodeStyle {
    pub(crate) font: TextStyle,
    pub(crate) scale: f32,
    pub(crate) wrap: bool,
    pub(crate) highlight: Color,
}

/// Transient selection and the live paint of its mounted code rows.
pub(crate) struct DiffSelection {
    value: Option<Selection>,
    gesture: Option<Gesture>,
    rows: RefCell<HashMap<Row, Weak<LinePaint>>>,
    viewport: Live<[f32; 4]>,
}

impl Default for DiffSelection {
    fn default() -> Self {
        Self {
            value: None,
            gesture: None,
            rows: RefCell::default(),
            viewport: Live::new([0.0; 4]),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Character,
    Word,
    Line,
}

struct Gesture {
    file: usize,
    start: Range<Point>,
    unit: Unit,
    previous: Option<Selection>,
}

impl Gesture {
    fn extend(&self, to: Range<Point>) -> Selection {
        let (anchor, focus) = if to.start < self.start.start {
            (self.start.end, to.start)
        } else {
            (self.start.start, self.start.end.max(to.end))
        };
        Selection {
            file: self.file,
            anchor,
            focus,
        }
    }
}

impl DiffSelection {
    pub(crate) fn clear(&mut self) {
        self.gesture = None;
        if self.value.take().is_some() {
            for row in self.rows.get_mut().values().filter_map(Weak::upgrade) {
                row.clear();
            }
        }
    }

    /// Installs the stable pointer capture and keyboard scope for the code column.
    pub(crate) fn pane(&self, pane: El, code_inset: f32) -> El {
        pane.drawn_rect_into(&self.viewport)
            .cursor(Cursor::Text)
            .focusable(true)
            .tab_index(-1)
            .no_focus_ring()
            .on_key(State::selection_key)
            .on_drag(move |state: &mut State, drag, tx| state.select_drag(drag, code_inset, tx))
            .on_secondary_press(move |state: &mut State, at, tx| {
                tx.live_only();
                let Some(selection) = state.selection.value else {
                    return;
                };
                let point = [at.x, at.y];
                if !state.in_code_column(point, code_inset)
                    || state
                        .code_at(point[1], None)
                        .is_none_or(|(file, _, _)| file != selection.file)
                {
                    return;
                }
                let Some(content) = state.selected_code() else {
                    return;
                };
                tx.popup_menu(
                    PopupMenu::new().item(PopupItem::new("copy", "Copy")),
                    at,
                    move |_: &mut State, choice, tx| {
                        if choice == Some("copy") {
                            write_copy(content, tx);
                        }
                        tx.live_only();
                    },
                );
            })
    }

    /// Keeps decorative diff rows and the gutter outside the selection gesture.
    pub(crate) fn background(el: El) -> El {
        el.cursor(Cursor::Default)
            .on_background_press_tx(|state: &mut State, tx| {
                state.selection.clear();
                tx.live_only();
            })
    }

    /// Wraps a code text in its paint layers. Syntax text still takes its usual layout.
    pub(crate) fn code(&self, id: Row, code: El, style: CodeStyle, spans: Vec<Span>) -> El {
        let paint = Rc::new(LinePaint::new(
            style.font,
            style.scale,
            style.wrap,
            spans,
            style.highlight,
        ));
        let mut rows = self.rows.borrow_mut();
        // Rune can briefly retain an old memo for an exit transition. Weak entries keep
        // neither those rows nor their shaped text alive once Rune releases them.
        if rows.len() >= 256 && rows.len().is_multiple_of(64) {
            rows.retain(|_, row| row.strong_count() != 0);
        }
        rows.insert(id, Rc::downgrade(&paint));
        col().w_full().children(paint.elements()).child(
            code.drawn_rect_into(&paint.drawn).on_layout_tx(
                move |state: &mut State, [_, _, width, _], tx| {
                    paint.width(width);
                    if let Some(line) = state.selection_line(id) {
                        let range = state
                            .selection
                            .value
                            .and_then(|value| value.bytes(id.0, id.1, line.text.len()));
                        paint.paint(line, range);
                    }
                    tx.live_only();
                },
            ),
        )
    }
}

impl State {
    fn selection_line(&self, (file, line): Row) -> Option<&Line> {
        match self.review.files.get(file)?.body(self.filter) {
            FileBody::Text(diff) => diff.lines.get(line),
            FileBody::Placeholder(_) => None,
        }
    }

    fn selection_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        let Some(value) = self
            .selection
            .value
            .filter(|_| tx.focused() == Some(SCROLL_KEY))
        else {
            return false;
        };
        let mods = event.mods;
        let extra_modifier = if cfg!(target_os = "macos") {
            mods.ctrl
        } else {
            mods.logo
        };
        let plain = !mods.shift && !mods.alt && !extra_modifier;
        if event.key == Key::Escape {
            self.selection.clear();
        } else if plain && event.is_command('a') {
            let all = Selection::all(&self.review, &self.rows, self.filter, value.file);
            self.set_selection(all);
        } else if plain && event.is_command('c') {
            let Some(content) = self.selected_code() else {
                return false;
            };
            write_copy(content, tx);
        } else {
            return false;
        }
        tx.live_only();
        true
    }

    fn selected_code(&self) -> Option<String> {
        self.selection
            .value?
            .copy(&self.review, &self.rows, self.filter)
    }

    fn set_selection(&mut self, value: Option<Selection>) {
        let old = self.selection.value;
        if old == value {
            return;
        }
        self.selection.value = value;
        for (&id, weak) in self.selection.rows.borrow().iter() {
            if ![old, value]
                .into_iter()
                .flatten()
                .any(|selection| selection.file == id.0)
            {
                continue;
            }
            if let Some(paint) = weak.upgrade()
                && let Some(line) = self.selection_line(id)
            {
                let range = value.and_then(|value| value.bytes(id.0, id.1, line.text.len()));
                paint.paint(line, range);
            }
        }
    }

    fn in_code_column(&self, [x, y]: [f32; 2], code_inset: f32) -> bool {
        let [left, top, width, height] = self.selection.viewport.get();
        let above = if self.sticky_file.is_some() {
            HEADER_HEIGHT
        } else {
            0.0
        };
        (left + code_inset..left + width).contains(&x) && (top + above..top + height).contains(&y)
    }

    fn code_at(
        &self,
        y: f32,
        nearest_in_file: Option<usize>,
    ) -> Option<(usize, usize, Rc<LinePaint>)> {
        let paints = self.selection.rows.borrow();
        self.rows
            .rows
            .get(self.visible_rows.clone())?
            .iter()
            .filter_map(|row| {
                let RowKind::Line(line) = row.kind else {
                    return None;
                };
                if nearest_in_file.is_some_and(|file| row.file != file) {
                    return None;
                }
                let paint = paints.get(&(row.file, line))?.upgrade()?;
                let [_, top, width, height] = paint.drawn.get();
                if width <= 0.0 || height <= 0.0 {
                    return None;
                }
                let distance = (top - y).max(y - (top + height)).max(0.0);
                if nearest_in_file.is_none() && !(top..top + height).contains(&y) {
                    return None;
                }
                Some((distance, (row.file, line, paint)))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, hit)| hit)
    }

    fn code_range(
        &self,
        (file, index, paint): (usize, usize, Rc<LinePaint>),
        at: [f32; 2],
        unit: Unit,
    ) -> Option<Range<Point>> {
        let line = self.selection_line((file, index))?;
        let bytes = if unit == Unit::Line {
            0..line.text.len()
        } else {
            paint.hit(line, at, unit == Unit::Word)
        };
        Some(
            Point {
                line: index,
                byte: bytes.start,
            }..Point {
                line: index,
                byte: bytes.end,
            },
        )
    }

    fn select_drag(&mut self, drag: &Drag, inset: f32, tx: &mut Tx) {
        let at = drag.window_point();
        tx.live_only();
        match drag.phase {
            Phase::Down => self.start_selection(at, inset, drag.clicks, tx.mods().shift),
            Phase::Move if self.selection.gesture.is_some() => {
                self.extend_selection(at);
                self.selection_scroll(at, inset, tx);
            }
            Phase::Up if drag.cancelled() => {
                if let Some(gesture) = self.selection.gesture.take() {
                    self.set_selection(gesture.previous);
                }
            }
            Phase::Up => {
                if drag.dx != 0.0 || drag.dy != 0.0 {
                    self.extend_selection(at);
                }
                self.selection.gesture = None;
            }
            Phase::Move => {}
        }
    }

    fn start_selection(&mut self, at: [f32; 2], inset: f32, clicks: u32, shift: bool) {
        let hit = self
            .in_code_column(at, inset)
            .then(|| self.code_at(at[1], None))
            .flatten();
        let Some(hit) = hit else {
            self.selection.clear();
            return;
        };
        let file = hit.0;
        let previous = self.selection.value;
        let unit = match (shift, clicks) {
            (true, _) | (_, 1) => Unit::Character,
            (_, 2) => Unit::Word,
            _ => Unit::Line,
        };
        let Some(to) = self.code_range(hit, at, unit) else {
            return;
        };
        let start = previous
            .filter(|sel| shift && sel.file == file)
            .map_or_else(|| to.clone(), |sel| sel.anchor..sel.anchor);
        let gesture = Gesture {
            file,
            start,
            unit,
            previous,
        };
        self.set_selection(Some(gesture.extend(to)));
        self.selection.gesture = Some(gesture);
    }

    fn extend_selection(&mut self, at: [f32; 2]) {
        let Some(gesture) = &self.selection.gesture else {
            return;
        };
        let Some(hit) = self.code_at(at[1], Some(gesture.file)) else {
            return;
        };
        if let Some(to) = self.code_range(hit, at, gesture.unit) {
            self.set_selection(Some(gesture.extend(to)));
        }
    }

    fn selection_scroll(&self, [px, py]: [f32; 2], inset: f32, tx: &mut Tx) {
        let Some(gesture) = &self.selection.gesture else {
            return;
        };
        let Some(file_rows) = self.rows.file_rows_range(gesture.file) else {
            return;
        };
        let [x, y, w, h] = self.selection.viewport.get();
        let at = self.scroll_position.get();
        let top = y + if self.sticky_file.is_some() {
            HEADER_HEIGHT
        } else {
            0.0
        };
        let mut dy = edge_delta(py, top, y + h);
        if (dy < 0.0 && self.visible_rows.start <= file_rows.start)
            || (dy > 0.0 && self.visible_rows.end >= file_rows.end)
        {
            dy = 0.0;
        }
        let dx = edge_delta(px, x + inset, x + w);
        let [old_x, old_y] = at.offset;
        let [max_x, max_y] = at.max;
        let to_x = (old_x + dx).clamp(0.0, max_x);
        let to_y = (old_y + dy).clamp(0.0, max_y);
        if (to_x - old_x).abs() > f32::EPSILON || (to_y - old_y).abs() > f32::EPSILON {
            tx.scroll_to_instant(SCROLL_KEY, to_x, to_y);
            tx.drag_repeat_after(REPEAT);
        }
    }
}

/// Copies text from the diff, with a clipboard trace for headless scripts.
pub(crate) fn write_copy(content: String, tx: &mut Tx) {
    // Headless scripts can't read the system clipboard. Trace the exact write they request.
    if cfg!(debug_assertions)
        && tx.tracing()
        && rune::tooling::headless_run().is_some()
        && let Ok(json) = serde_json::to_string(&content)
    {
        tx.trace_event(format_args!("diff-copy {json}"));
    }
    tx.copy(content);
}

fn edge_delta(at: f32, start: f32, end: f32) -> f32 {
    const BAND: f32 = 28.0;
    const SPEED: f32 = 18.0;
    let toward_start = ((start + BAND - at) / BAND).clamp(0.0, 1.0);
    let toward_end = ((at - (end - BAND)) / BAND).clamp(0.0, 1.0);
    SPEED * (toward_end - toward_start)
}
