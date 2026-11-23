use ghostty_vt::{
    Terminal,
    screen::GridRef,
    selection::{
        FormatOptions,
        gesture::{Behavior, Behaviors, DragEvent, Geometry, PressEvent, ReleaseEvent},
    },
    terminal::{Point, PointCoordinate},
};

use super::{Emulator, VtError};

// Ghostty's gesture geometry uses integer pixels. Use a fixed number of units per
// cell so fractional display cell widths still give the correct drag boundaries.
const CELL_UNITS: u32 = 100;

pub(super) struct Gesture {
    state: ghostty_vt::selection::gesture::Gesture,
    press: PressEvent,
    drag: DragEvent,
    release: ReleaseEvent,
}

impl Gesture {
    pub(super) fn new() -> Result<Self, VtError> {
        Ok(Self {
            state: ghostty_vt::selection::gesture::Gesture::new()?,
            press: PressEvent::new()?,
            drag: DragEvent::new()?,
            release: ReleaseEvent::new()?,
        })
    }
}

impl Emulator {
    /// Begins a selection at a position in viewport cells, including fractional offsets.
    /// Uses the host's click count for word and line selection.
    ///
    /// # Errors
    /// Returns Ghostty's error if the position or gesture is invalid.
    pub fn start_selection(&mut self, [x, y]: [f64; 2], clicks: u32) -> Result<(), VtError> {
        let terminal = &self.terminal;
        let gesture = &mut self.selection;
        let grid_ref = viewport_ref(terminal, x, y)?;
        let behavior = match clicks {
            0 | 1 => Behavior::Cell,
            2 => Behavior::Word,
            _ => Behavior::Line,
        };
        // Leave this event untimed so Ghostty uses the click behavior chosen by Rune.
        let selection = gesture
            .press
            .set_position(x * f64::from(CELL_UNITS), y * f64::from(CELL_UNITS))?
            .set_behaviors(&Behaviors::new().with_single_click_behavior(behavior))?
            .apply(&mut gesture.state, terminal, grid_ref)?;
        terminal.set_selection(selection.as_ref())?;
        Ok(())
    }

    /// Extends the active selection gesture to a position in viewport cells.
    ///
    /// # Errors
    /// Returns Ghostty's error if the position or gesture is invalid.
    pub fn drag_selection(&mut self, [x, y]: [f64; 2]) -> Result<(), VtError> {
        let terminal = &self.terminal;
        let gesture = &mut self.selection;
        let grid_ref = viewport_ref(terminal, x, y)?;
        let geometry = Geometry {
            columns: u32::from(terminal.cols()?),
            cell_width: CELL_UNITS,
            padding_left: 0,
            screen_height: u32::from(terminal.rows()?) * CELL_UNITS,
        };
        let selection = gesture
            .drag
            .set_position(x * f64::from(CELL_UNITS), y * f64::from(CELL_UNITS))?
            .apply(&mut gesture.state, terminal, grid_ref, geometry)?;
        if let Some(selection) = &selection {
            terminal.set_selection(Some(selection))?;
        }
        Ok(())
    }

    /// Ends the pointer gesture while preserving the installed selection.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot release the gesture.
    pub fn finish_selection(&mut self) -> Result<(), VtError> {
        let gesture = &mut self.selection;
        let result = gesture
            .release
            .apply(&mut gesture.state, &self.terminal, None);
        gesture.state.reset(&self.terminal);
        result
    }

    /// Clears the active selection and cancels any selection gesture.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot clear the selection.
    pub fn clear_selection(&mut self) -> Result<(), VtError> {
        self.selection.state.reset(&self.terminal);
        self.terminal.set_selection(None)?;
        Ok(())
    }

    /// Returns the selected text, trimming line endings and joining soft wraps.
    ///
    /// # Errors
    /// Returns Ghostty's error if the selection cannot be formatted.
    pub fn selected_text(&self) -> Result<Option<String>, VtError> {
        let options = FormatOptions::new().with_unwrap(true).with_trim(true);
        Ok(self
            .terminal
            .format_selection_alloc(options)?
            .map(|bytes| trim_lines(&String::from_utf8_lossy(&bytes))))
    }
}

fn trim_lines(text: &str) -> String {
    // Ghostty trims padding spaces. Trim any remaining Unicode whitespace at hard line endings.
    let mut trimmed = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index != 0 {
            trimmed.push('\n');
        }
        trimmed.push_str(line.trim_end());
    }
    trimmed
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Viewport coordinates are validated before taking their integer cell index"
)]
fn viewport_ref(terminal: &Terminal, x: f64, y: f64) -> Result<GridRef<'_>, VtError> {
    if !(0.0..f64::from(terminal.cols()?)).contains(&x)
        || !(0.0..f64::from(terminal.rows()?)).contains(&y)
    {
        return Err(VtError::InvalidValue);
    }
    terminal.grid_ref(Point::Viewport(PointCoordinate {
        x: x as u16,
        y: y as u32,
    }))
}
