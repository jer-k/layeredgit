use std::{ops::Range, time::Duration};

use rune::{Drag, Mods, Phase, PointerButton, PointerEvent, PopupItem, PopupMenu, Tx, WheelEvent};
use term::{key, mouse};

use super::{CELL_HEIGHT, CELL_WIDTH, Grid, State, TerminalPane};

const SCROLL_REPEAT: Duration = Duration::from_millis(50);

#[derive(Default)]
pub(super) struct Pointer {
    capture: Option<Capture>,
    wheel: [f32; 2],
}

#[derive(Clone, Copy)]
enum Capture {
    Selection,
    Program(mouse::Button),
}

impl Pointer {
    pub(super) fn cancel_selection(&mut self) {
        if matches!(self.capture, Some(Capture::Selection)) {
            self.capture = None;
        }
    }
}

impl TerminalPane {
    fn grid_position(&self, at: [f32; 2]) -> Option<[f32; 2]> {
        grid_position(at, self.visible_rows(), self.columns())
    }

    fn mouse_position(&self, at: [f32; 2], mods: Mods) -> Option<mouse::Position> {
        let session = self.session.as_ref()?;
        if mods.shift || session.source.is_none() || !session.emulator.is_mouse_tracking() {
            return None;
        }
        active_position(self.grid_position(at)?, &session.viewport_grid().ok()?)
    }

    fn report_mouse(
        &mut self,
        position: mouse::Position,
        action: mouse::Action,
        button: Option<mouse::Button>,
        mods: Mods,
        count: usize,
    ) {
        if let Some(session) = &mut self.session
            && let Some(source) = &session.source
            && let Ok(bytes) =
                session
                    .emulator
                    .encode_mouse(action, button, mouse_mods(mods), position)
            && !bytes.is_empty()
        {
            source.source().write(if count == 1 {
                bytes
            } else {
                bytes.repeat(count)
            });
        }
    }

    fn begin_drag(&mut self, drag: &Drag, tx: &mut Tx) -> bool {
        let mods = tx.mods();
        let button = match tx.button() {
            PointerButton::Primary => mouse::Button::Left,
            PointerButton::Middle => mouse::Button::Middle,
            _ => return false,
        };
        let at = [drag.x, drag.y];
        if let Some(position) = self.mouse_position(at, mods) {
            self.pointer.capture = Some(Capture::Program(button));
            self.report_mouse(position, mouse::Action::Press, Some(button), mods, 1);
            return button == mouse::Button::Left && self.clear_pointer_selection();
        }
        let Some(position) = self.grid_position(at).map(|point| point.map(f64::from)) else {
            return false;
        };
        let Some(session) = &mut self.session else {
            return false;
        };
        if button != mouse::Button::Left {
            return false;
        }
        // Shift gives the terminal's selection gesture to the user while an app tracks the mouse.
        if !mods.shift && session.source.is_some() && session.emulator.is_mouse_tracking() {
            return self.clear_pointer_selection();
        }
        if session
            .emulator
            .start_selection(position, drag.clicks)
            .is_err()
        {
            return false;
        }
        self.pointer.capture = Some(Capture::Selection);
        self.refresh_screen()
    }

    fn clear_pointer_selection(&mut self) -> bool {
        if let Some(session) = &mut self.session {
            let _ = session.emulator.clear_selection();
        }
        self.pointer.cancel_selection();
        self.refresh_screen()
    }

    fn select_drag(&mut self, drag: &Drag, tx: &mut Tx) -> bool {
        if drag.cancelled() {
            return self.clear_pointer_selection();
        }
        let mut changed = false;
        if drag.phase == Phase::Move {
            let rows = autoscroll_rows(drag.y, drag.h);
            if rows != 0 && self.scroll_rows(rows) {
                changed = true;
                tx.drag_repeat_after(SCROLL_REPEAT);
            }
        }
        let position = self
            .grid_position([drag.x, drag.y])
            .map(|point| point.map(f64::from));
        let Some(session) = &mut self.session else {
            return changed;
        };
        if (drag.phase == Phase::Move || drag.dx != 0.0 || drag.dy != 0.0)
            && let Some(position) = position
        {
            let _ = session.emulator.drag_selection(position);
            changed |= self.refresh_screen();
        }
        if drag.phase == Phase::Up {
            if let Some(session) = &mut self.session {
                let _ = session.emulator.finish_selection();
            }
            self.pointer.capture = None;
        }
        changed
    }

    pub(super) fn copy_selection(&self, tx: &mut Tx) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        copy(text, tx);
        true
    }

    fn selected_text(&self) -> Option<String> {
        self.session.as_ref()?.emulator.selected_text().ok()?
    }
}

impl State {
    pub(crate) fn terminal_drag(&mut self, drag: &mut Drag, tx: &mut Tx) {
        let pane = &mut self.terminal;
        let changed = match (drag.phase, pane.pointer.capture) {
            (Phase::Down, _) => pane.begin_drag(drag, tx),
            (_, Some(Capture::Selection)) => pane.select_drag(drag, tx),
            (phase, Some(Capture::Program(button))) => {
                let mods = tx.mods();
                let action = if phase == Phase::Up {
                    pane.pointer.capture = None;
                    mouse::Action::Release
                } else {
                    mouse::Action::Motion
                };
                // Capture the routing decision on press, even if Shift changes before release.
                if let Some(position) = pane.mouse_position(
                    [drag.x, drag.y],
                    Mods {
                        shift: false,
                        ..mods
                    },
                ) {
                    pane.report_mouse(position, action, Some(button), mods, 1);
                }
                false
            }
            (_, None) => false,
        };
        if changed {
            tx.instant();
        } else {
            tx.live_only();
        }
    }

    pub(crate) fn terminal_pointer_move(&mut self, point: PointerEvent, tx: &mut Tx) {
        let pane = &mut self.terminal;
        let mods = tx.mods();
        if pane.pointer.capture.is_none()
            && let Some(position) = pane.mouse_position([point.x, point.y], mods)
        {
            pane.report_mouse(position, mouse::Action::Motion, None, mods, 1);
        }
        tx.live_only();
    }

    pub(crate) fn terminal_context_menu(&mut self, point: PointerEvent, tx: &mut Tx) {
        let pane = &mut self.terminal;
        let mods = tx.mods();
        if let Some(position) = pane.mouse_position([point.x, point.y], mods) {
            // Rune exposes secondary clicks on release, so send the pair together.
            for action in [mouse::Action::Press, mouse::Action::Release] {
                pane.report_mouse(position, action, Some(mouse::Button::Right), mods, 1);
            }
        } else if let Some(text) = pane.selected_text() {
            tx.popup_menu(
                PopupMenu::new().item(PopupItem::new("copy", "Copy")),
                point.window(),
                move |_: &mut State, choice, tx| {
                    if choice == Some("copy") {
                        copy(text, tx);
                    }
                    tx.live_only();
                },
            );
        }
        tx.live_only();
    }

    pub(crate) fn terminal_wheel(&mut self, wheel: WheelEvent, tx: &mut Tx) -> bool {
        let pane = &mut self.terminal;
        if let Some(position) = pane.mouse_position([wheel.x, wheel.y], wheel.mods)
            && !matches!(pane.pointer.capture, Some(Capture::Selection))
        {
            pane.wheel_remainder = 0.0;
            for (axis, delta, positive, negative) in [
                (0, wheel.dx, mouse::Button::Six, mouse::Button::Seven),
                (1, wheel.dy, mouse::Button::Four, mouse::Button::Five),
            ] {
                let rows = wheel_steps(&mut pane.pointer.wheel[axis], delta, !wheel.precise);
                if rows != 0 {
                    pane.report_mouse(
                        position,
                        mouse::Action::Press,
                        Some(if rows > 0 { positive } else { negative }),
                        wheel.mods,
                        usize::from(rows.unsigned_abs()),
                    );
                }
            }
        } else {
            pane.pointer.wheel = [0.0; 2];
            // Positive dy moves the content down, revealing older rows.
            let rows = wheel_steps(&mut pane.wheel_remainder, -wheel.dy, false);
            if rows != 0 && pane.scroll_rows(isize::from(rows)) {
                return true;
            }
        }
        tx.live_only();
        true
    }
}

fn copy(text: String, tx: &mut Tx) {
    if cfg!(debug_assertions)
        && tx.tracing()
        && rune::tooling::headless_run().is_some()
        && let Ok(json) = serde_json::to_string(&text)
    {
        tx.trace_event(format_args!("terminal-copy {json}"));
    }
    tx.copy(text);
}

fn grid_position([x, y]: [f32; 2], rows: Range<usize>, columns: u16) -> Option<[f32; 2]> {
    let first = u16::try_from(rows.start).ok()?;
    let height = u16::try_from(rows.len()).ok()?;
    if columns == 0 || height == 0 {
        return None;
    }
    // Clamp to the inside edge of the last cell, preserving the fraction for drag selection.
    Some([
        (x / CELL_WIDTH).clamp(0.0, f32::from(columns) - 0.01),
        f32::from(first) + (y / CELL_HEIGHT).clamp(0.0, f32::from(height) - 0.01),
    ])
}

fn active_position([x, y]: [f32; 2], grid: &Grid) -> Option<mouse::Position> {
    let history = u16::try_from(grid.history.saturating_sub(grid.offset)).ok()?;
    let y = y - f32::from(history);
    (y >= 0.0).then_some(mouse::Position { x, y })
}

fn autoscroll_rows(y: f32, height: f32) -> isize {
    let overshoot = if y < 0.0 { y } else { (y - height).max(0.0) };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Autoscroll is bounded to five rows in either direction"
    )]
    let rows = (overshoot / CELL_HEIGHT).abs().ceil().min(5.0) as isize;
    if overshoot < 0.0 { -rows } else { rows }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "Scroll by whole rows and carry the fractional remainder over"
)]
fn wheel_steps(remainder: &mut f32, delta: f32, discrete: bool) -> i16 {
    if discrete {
        *remainder = 0.0;
        return (delta / CELL_HEIGHT)
            .abs()
            .ceil()
            .copysign(delta)
            .clamp(-1000.0, 1000.0) as i16;
    }
    *remainder += delta / CELL_HEIGHT;
    let steps = remainder.trunc().clamp(-1000.0, 1000.0) as i16;
    *remainder -= f32::from(steps);
    steps
}

fn mouse_mods(mods: Mods) -> key::Mods {
    let mut encoded = key::Mods::empty();
    encoded.set(key::Mods::SHIFT, mods.shift);
    encoded.set(key::Mods::CTRL, mods.ctrl);
    encoded.set(key::Mods::ALT, mods.alt);
    encoded.set(key::Mods::SUPER, mods.logo || (mods.cmd && !mods.ctrl));
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_clamps_points_to_the_visible_slice() {
        let point = grid_position([CELL_WIDTH * 2.5, CELL_HEIGHT * 3.25], 42..53, 80);
        assert_eq!(point, Some([2.5, 45.25]));
        assert_eq!(grid_position([-5.0, -6.0], 42..53, 80), Some([0.0, 42.0]));
        let [x, y] = grid_position([2000.0, 400.0], 42..53, 80).unwrap();
        assert!((79.9..80.0).contains(&x) && (52.9..53.0).contains(&y));
        assert!(grid_position([5.0, 10.0], 0..0, 80).is_none());
    }

    #[test]
    fn reports_active_rows_below_the_scrollback_offset() {
        let grid = Grid {
            rows: 53,
            offset: 91,
            history: 100,
            cursor: 52,
        };
        assert!(active_position([2.5, 8.5], &grid).is_none());
        let position = active_position([2.5, 9.5], &grid).unwrap();
        assert!((position.x - 2.5).abs() < f32::EPSILON);
        assert!((position.y - 0.5).abs() < f32::EPSILON);

        let grid = Grid {
            offset: 100,
            ..grid
        };
        let point = grid_position([CELL_WIDTH, 0.0], 42..53, 80).unwrap();
        let position = active_position(point, &grid).unwrap();
        assert!((position.y - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn autoscrolls_only_outside_the_pane() {
        assert_eq!(autoscroll_rows(50.0, 220.0), 0);
        assert_eq!(autoscroll_rows(-1.0, 220.0), -1);
        assert_eq!(autoscroll_rows(241.0, 220.0), 2);
        assert_eq!(autoscroll_rows(1000.0, 220.0), 5);
    }
}
