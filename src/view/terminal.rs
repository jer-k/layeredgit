use std::borrow::Cow;

use rune::{Color, El, GrowthSpec, ScrollSpec, TextStyle, col, color, el, memo, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit};
use term::{Cell, CursorVisualStyle, RgbColor, Row, Size};

use crate::state::{
    State,
    terminal::{CELL_HEIGHT, CELL_WIDTH, SCREEN_KEY},
};

const FONT_SIZE: f32 = 12.0;
const PADDING: f32 = 16.0;
const GRID_HEIGHT: f32 = 220.0;

pub(super) fn toggle(state: &State, kit: &Kit) -> El {
    let action = if !state.terminal.open {
        "Show terminal"
    } else if state.terminal.ended() {
        "Restart terminal"
    } else {
        "Hide terminal"
    };
    Button::new("Terminal")
        .variant(if state.terminal.open {
            ButtonVariant::Neutral
        } else {
            ButtonVariant::Ghost
        })
        .control_size(ControlSize::Small)
        .on_press_tx(State::toggle_terminal)
        .build(kit)
        .key("terminal-toggle")
        .label(action)
        .help(format!("{action} (Ctrl+`)"))
}

pub(super) fn pane(state: &State, kit: &Kit, scale: f32) -> El {
    let pane = &state.terminal;
    let screen = pane.screen();
    let bg = screen
        .and_then(|screen| screen.background)
        .map_or(kit.c.bg_base, color_from_rgb);
    let fg = screen
        .and_then(|screen| screen.foreground)
        .map_or(kit.c.text_base, color_from_rgb);
    let font = TextStyle {
        size: FONT_SIZE,
        line_height: CELL_HEIGHT,
        ..kit.ty.mono
    };
    let look = RowStyle {
        foreground: fg,
        background: bg,
        font,
    };
    let dark = kit.dark;
    let canvas = col()
        .key(SCREEN_KEY)
        .role("group")
        .label("Terminal output and input")
        .focusable(true)
        .no_focus_ring()
        .on_key(State::terminal_key)
        .on_key_up(State::terminal_key_up)
        .on_wheel(|state: &mut State, wheel, tx| state.terminal_scroll(wheel.dy, tx))
        .h(GRID_HEIGHT)
        .w_full()
        .child(
            // Remount the geometry listener when appearance or backing scale changes.
            // The enclosing focus target and the viewport keep their keys.
            el().key(format!("terminal-geometry-{dark}-{}", scale.to_bits()))
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .inert()
                .on_layout_tx(move |state: &mut State, rect, tx| {
                    state.layout_terminal(grid_size(rect[2], rect[3], scale), dark, tx);
                }),
        )
        .child(
            col()
                .key("terminal-viewport")
                .w_full()
                .h_full()
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .on_wheel(|state: &mut State, wheel, tx| state.terminal_scroll(wheel.dy, tx))
                .child(
                    col()
                        .key("terminal-rows")
                        .w_full()
                        .growth(GrowthSpec::default())
                        .virtual_rows(
                            screen.map_or(0, |screen| screen.rows.len()),
                            CELL_HEIGHT,
                            |state: &State, index| {
                                terminal_row(state, index)
                                    .map_or(Cow::Borrowed(""), |row| Cow::Borrowed(&row.key))
                            },
                            move |state: &State, index| {
                                terminal_row(state, index).map_or_else(
                                    || el().h(CELL_HEIGHT).w_full(),
                                    |row| {
                                        memo(
                                            &row.key,
                                            (state.terminal.id, row.revision, look),
                                            || look.draw_row(row),
                                        )
                                    },
                                )
                            },
                        )
                        .virtual_rows_version((pane.id, pane.version)),
                ),
        )
        .children(screen.and_then(|screen| {
            let cursor = screen.cursor.filter(|_| !pane.ended())?;
            let width = terminal_row(state, usize::from(cursor.row))
                .and_then(|row| row.cells.iter().find(|cell| cell.column == cursor.column))
                .map_or(1.0, |cell| f32::from(cell.width))
                * CELL_WIDTH;
            let ink = screen.cursor_color.map_or(fg, color_from_rgb);
            Some(draw_cursor(state, cursor, width, ink, look))
        }));
    col()
        .key("terminal-pane")
        .instant_changes()
        .w_full()
        .fill(bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], kit.c.border_muted)
        .child(header(state, kit))
        .child(col().px(PADDING).py(8.0).w_full().child(canvas))
}

fn header(state: &State, kit: &Kit) -> El {
    row()
        .key("terminal-header")
        .h(38.0)
        .px(PADDING)
        .gap(10.0)
        .items_center()
        .fill(kit.c.layer_01)
        .border_sides([0.0, 0.0, 1.0, 0.0], kit.c.border_muted)
        .child(
            text("Terminal")
                .font(kit.ty.ui_medium)
                .color(kit.c.text_base),
        )
        .children(state.terminal.status().map(|status| {
            text(status)
                .key("terminal-status")
                .role("status")
                .font(kit.ty.small)
                .color(kit.c.text_muted)
                .shrink()
                .truncate_middle()
        }))
        .child(el().grow(1.0))
        .child(
            IconButton::new(kit.glyph.close)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label("Hide terminal")
                .on_press_tx(State::hide_terminal)
                .build(kit)
                .key("terminal-hide"),
        )
}

#[derive(Clone, Copy, Hash)]
struct RowStyle {
    foreground: Color,
    background: Color,
    font: TextStyle,
}

impl RowStyle {
    fn draw_row(self, row: &Row) -> El {
        el().w_full()
            .h(CELL_HEIGHT)
            .children(row.cells.iter().map(|cell| self.draw_cell(cell)))
    }

    fn draw_cell(self, cell: &Cell) -> El {
        let mut foreground = cell.foreground.map_or(self.foreground, color_from_rgb);
        let mut background = cell.background.map_or(self.background, color_from_rgb);
        if cell.inverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        if cell.faint {
            foreground = foreground.mix(background, 0.45);
        }
        let width = f32::from(cell.width) * CELL_WIDTH;
        let glyph = (!cell.text.is_empty()).then(|| {
            text(&cell.text)
                .font(TextStyle {
                    weight: if cell.bold { 700.0 } else { self.font.weight },
                    italic: cell.italic,
                    ..self.font
                })
                .color(foreground)
                .single_line()
        });
        let underline = cell
            .underline
            .then(|| el().absolute(0.0, 16.0).size(width, 1.0).fill(foreground));
        let strike = cell
            .strikethrough
            .then(|| el().absolute(0.0, 10.0).size(width, 1.0).fill(foreground));
        let mut item = el()
            .absolute(f32::from(cell.column) * CELL_WIDTH, 0.0)
            .size(width, CELL_HEIGHT);
        if cell.inverse || cell.background.is_some() {
            item = item.fill(background);
        }
        item.children(glyph).children(underline).children(strike)
    }
}

fn draw_cursor(state: &State, cursor: term::Cursor, width: f32, ink: Color, look: RowStyle) -> El {
    let x = f32::from(cursor.column) * CELL_WIDTH;
    let y = f32::from(cursor.row) * CELL_HEIGHT;
    let focused = state.terminal.focused;
    let (width, height, offset) = match cursor.style {
        CursorVisualStyle::Bar if focused => (2.0, 16.0, 2.0),
        CursorVisualStyle::Underline if focused => (width, 2.0, 16.0),
        _ => (width, 16.0, 2.0),
    };
    let caret = el()
        .key("terminal-cursor")
        .absolute(x, y + offset)
        .size(width, height)
        .inert();
    let caret = if !focused || cursor.style == CursorVisualStyle::BlockHollow {
        caret.border(1.0, ink)
    } else if cursor.style == CursorVisualStyle::Block {
        caret.fill(ink.alpha(0.38)).border(1.0, ink)
    } else {
        caret.fill(ink)
    };
    let caret = if focused { caret.ime_cursor() } else { caret };
    if state.terminal.preedit.is_empty() {
        caret
    } else {
        el().absolute(x, y)
            .child(caret.absolute(0.0, offset))
            .child(
                text(&state.terminal.preedit)
                    .single_line()
                    .font(look.font)
                    .color(look.foreground)
                    .fill(look.background)
                    .border_sides([0.0, 0.0, 1.0, 0.0], ink),
            )
    }
}

fn terminal_row(state: &State, index: usize) -> Option<&Row> {
    state.terminal.screen()?.rows.get(index)?.as_deref()
}

fn color_from_rgb(rgb: RgbColor) -> Color {
    let channel = |value| color::srgb_decode(f32::from(value) / 255.0);
    Color {
        r: channel(rgb.r),
        g: channel(rgb.g),
        b: channel(rgb.b),
        a: 1.0,
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Clamp grid geometry to the PTY's u16 dimensions"
)]
fn grid_size(width: f32, height: f32, scale: f32) -> Size {
    let dimension = |value: f32| value.clamp(1.0, f32::from(u16::MAX)) as u16;
    Size {
        cols: dimension((width / CELL_WIDTH).floor()),
        rows: dimension((height / CELL_HEIGHT).floor()),
        cell_width: dimension((CELL_WIDTH * scale).round()),
        cell_height: dimension((CELL_HEIGHT * scale).round()),
    }
}
