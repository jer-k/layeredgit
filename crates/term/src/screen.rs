use std::{collections::HashMap, rc::Rc};

use ghostty_vt::{
    RenderState, Terminal,
    render::{CellIterator, CursorVisualStyle, Dirty, RowId, RowIteration, RowIterator},
    screen::CellWide,
    style::{RgbColor, Underline},
};

use crate::VtError;

/// A styled glyph or painted blank in the terminal grid.
#[derive(Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "SGR attributes can be combined independently"
)]
pub struct Cell {
    /// Zero-based column of the leading cell.
    pub column: u16,
    /// Number of columns occupied by the grapheme.
    pub width: u16,
    /// The complete grapheme, possibly empty.
    pub text: String,
    /// Explicit foreground colour, if any.
    pub foreground: Option<RgbColor>,
    /// Explicit background colour, if any.
    pub background: Option<RgbColor>,
    /// Render with heavier weight.
    pub bold: bool,
    /// Render in italic.
    pub italic: bool,
    /// Render with reduced intensity.
    pub faint: bool,
    /// Exchange the foreground and background.
    pub inverse: bool,
    /// Draw a line beneath the text.
    pub underline: bool,
    /// Draw a line through the text.
    pub strikethrough: bool,
}

/// A cached viewport row.
pub struct Row {
    /// A stable key while the row remains in the viewport.
    pub key: String,
    /// Changes when the row's contents change.
    pub revision: u64,
    /// Cells that have visible content or styling.
    pub cells: Vec<Cell>,
}

/// Position and appearance of a visible terminal cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Zero-based column.
    pub column: u16,
    /// Zero-based viewport row.
    pub row: u16,
    /// The shape requested by the program.
    pub style: CursorVisualStyle,
}

/// The viewport with colours and visible cursor resolved from the terminal.
#[derive(Default)]
pub struct Screen {
    /// Cached rows in viewport order.
    pub rows: Vec<Option<Rc<Row>>>,
    /// Current default foreground.
    pub foreground: Option<RgbColor>,
    /// Current default background.
    pub background: Option<RgbColor>,
    /// Explicit cursor colour, if any.
    pub cursor_color: Option<RgbColor>,
    /// Cursor, when enabled and inside the viewport.
    pub cursor: Option<Cursor>,
}

pub(super) struct Renderer {
    state: RenderState,
    row_iter: RowIterator,
    cell_iter: CellIterator,
    rows: HashMap<RowId, Rc<Row>>,
    revision: u64,
    pub(super) screen: Screen,
}

impl Renderer {
    pub(super) fn new() -> Result<Self, VtError> {
        Ok(Self {
            state: RenderState::new()?,
            row_iter: RowIterator::new()?,
            cell_iter: CellIterator::new()?,
            rows: HashMap::new(),
            revision: 0,
            screen: Screen::default(),
        })
    }

    pub(super) fn update(&mut self, terminal: &Terminal) -> Result<bool, VtError> {
        let snapshot = self.state.update(terminal)?;
        let colors = snapshot.colors()?;
        let raw_cursor = snapshot.cursor()?;
        let cursor = raw_cursor
            .viewport
            .filter(|_| raw_cursor.visible)
            .map(|pos| Cursor {
                column: pos.x.saturating_sub(u16::from(pos.at_wide_tail)),
                row: pos.y,
                style: raw_cursor.visual_style,
            });
        let colors_changed = self.screen.foreground != Some(colors.foreground)
            || self.screen.background != Some(colors.background)
            || self.screen.cursor_color != colors.cursor;
        let cursor_moved = self.screen.cursor != cursor;
        self.screen.foreground = Some(colors.foreground);
        self.screen.background = Some(colors.background);
        self.screen.cursor_color = colors.cursor;
        self.screen.cursor = cursor;

        let dirty = snapshot.dirty()?;
        if dirty == Dirty::Clean && !colors_changed {
            return Ok(cursor_moved);
        }

        self.revision = self.revision.wrapping_add(1);
        let mut rows = self.row_iter.update(&snapshot)?;
        let mut next = HashMap::new();
        let mut viewport = vec![None; usize::from(snapshot.rows()?)];
        while let Some(row) = rows.next() {
            let y = row.viewport_y()?;
            let Some(slot) = usize::try_from(y).ok().and_then(|y| viewport.get_mut(y)) else {
                continue;
            };
            let id = row.id()?;
            let cached = self.rows.get(&id);
            let refresh = dirty == Dirty::Full || colors_changed || row.dirty()?;
            let rendered = match cached {
                Some(cached) if !refresh => Rc::clone(cached),
                _ => {
                    let cells = cells(&mut self.cell_iter, row)?;
                    if let Some(cached) = cached.filter(|cached| cached.cells == cells) {
                        Rc::clone(cached)
                    } else {
                        let [page, offset] = id.0;
                        let key = if id.is_none() {
                            format!("terminal-row-{y}")
                        } else {
                            format!("terminal-row-{page:x}-{offset:x}")
                        };
                        Rc::new(Row {
                            key,
                            revision: self.revision,
                            cells,
                        })
                    }
                }
            };
            if !id.is_none() {
                next.insert(id, Rc::clone(&rendered));
            }
            *slot = Some(rendered);
        }
        snapshot.clean()?;
        let changed = self.screen.rows.len() != viewport.len()
            || self
                .screen
                .rows
                .iter()
                .zip(&viewport)
                .any(|(old, new)| match (old, new) {
                    (Some(old), Some(new)) => !Rc::ptr_eq(old, new),
                    (None, None) => false,
                    _ => true,
                });
        self.rows = next;
        self.screen.rows = viewport;
        Ok(changed || colors_changed || cursor_moved)
    }
}

fn cells(iter: &mut CellIterator, row: &RowIteration<'_>) -> Result<Vec<Cell>, VtError> {
    let mut cells = iter.update(row)?;
    let mut result = Vec::new();
    let mut column = 0;
    while let Some(cell) = cells.next() {
        let x = column;
        column += 1;
        let raw = cell.raw_cell()?;
        let width = match raw.wide()? {
            CellWide::Wide => 2,
            CellWide::SpacerTail => continue,
            _ => 1,
        };
        let mut text = String::new();
        cell.graphemes_utf8(&mut text)?;
        let background = cell.bg_color()?;
        let style = cell.style()?;
        if style.invisible {
            text.clear();
        }
        if text.is_empty() && background.is_none() && !style.inverse {
            continue;
        }
        result.push(Cell {
            column: x,
            width,
            text,
            foreground: cell.fg_color()?,
            background,
            bold: style.bold,
            italic: style.italic,
            faint: style.faint,
            inverse: style.inverse,
            underline: style.underline != Underline::None && !style.invisible,
            strikethrough: style.strikethrough && !style.invisible,
        });
    }
    Ok(result)
}
