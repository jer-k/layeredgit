use ghostty_vt::{
    Terminal, TerminalOptions, focus, key, paste, snapshot,
    style::{Palette, RgbColor},
    terminal::{ColorScheme, Event, Mode, ScrollViewport, Scrollbar},
};

use crate::{Screen, Size, Source, VtError, screen::Renderer};

mod mouse;
mod selection;

/// Owns the thread-affine Ghostty handles and their cached viewport.
pub struct Emulator {
    terminal: Terminal,
    renderer: Renderer,
    encoder: key::Encoder,
    event: key::Event,
    mouse: mouse::Mouse,
    selection: selection::Gesture,
}

impl Emulator {
    /// Creates a blank terminal with bounded scrollback.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot create the handles or set the dimensions.
    pub fn new(size: Size) -> Result<Self, VtError> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback: 10 * 1024 * 1024,
        })?;
        terminal.set_size_reports(true);
        terminal.set_terminfo_name(Some(ghostty_vt::terminfo::TERM))?;
        terminal.set_clipboard_write_policy(ghostty_vt::terminal::ClipboardWritePolicy::Deny);
        let mut emulator = Self {
            terminal,
            renderer: Renderer::new()?,
            encoder: key::Encoder::new()?,
            event: key::Event::new()?,
            mouse: mouse::Mouse::new()?,
            selection: selection::Gesture::new()?,
        };
        emulator.resize(size)?;
        Ok(emulator)
    }

    /// Restores a Rex Ghostty snapshot, including the parser state and scrollback.
    ///
    /// # Errors
    /// Returns Ghostty's error if the snapshot cannot be decoded or configured.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, VtError> {
        let mut terminal = snapshot::Decoder::new(bytes)?.decode()?;
        terminal.set_size_reports(false);
        terminal.set_clipboard_write_policy(ghostty_vt::terminal::ClipboardWritePolicy::Deny);
        terminal.set_default_cursor_color(None)?;
        terminal.scroll_viewport(ScrollViewport::Bottom);
        Ok(Self {
            terminal,
            renderer: Renderer::new()?,
            encoder: key::Encoder::new()?,
            event: key::Event::new()?,
            mouse: mouse::Mouse::new()?,
            selection: selection::Gesture::new()?,
        })
    }

    /// Discards VT replies from the local emulator when the source handles them.
    pub fn discard_responses(&mut self) {
        drop(self.terminal.events());
    }

    /// Applies theme defaults while preserving colours explicitly set by the program.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot apply a colour.
    pub fn theme(
        &mut self,
        dark: bool,
        foreground: RgbColor,
        background: RgbColor,
        ansi: [RgbColor; 16],
    ) -> Result<(), VtError> {
        let mut palette = Palette::default();
        for (slot, color) in palette.0.iter_mut().zip(ansi) {
            *slot = color;
        }
        self.terminal
            .set_default_fg_color(Some(foreground))?
            .set_default_bg_color(Some(background))?
            .set_default_color_palette(Some(palette))?;
        self.terminal.set_color_scheme(Some(if dark {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        }));
        Ok(())
    }

    /// Feeds program output to the emulator.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.terminal.vt_write(bytes);
    }

    /// Sends terminal query responses and mode reports to the program.
    pub fn flush_responses(&mut self, source: &dyn Source) {
        for event in self.terminal.events() {
            if let Event::PtyWrite(bytes) = event {
                source.write(bytes);
            }
        }
    }

    /// Resizes the emulator's grid.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot resize the grid.
    pub fn resize(&mut self, size: Size) -> Result<(), VtError> {
        self.terminal.resize(
            size.cols,
            size.rows,
            u32::from(size.cell_width),
            u32::from(size.cell_height),
        )
    }

    /// Scrolls to a row offset from the top of scrollback, clamped to the active screen.
    pub fn scroll_to_row(&mut self, row: usize) {
        self.terminal.scroll_viewport(ScrollViewport::Row(row));
    }

    /// Returns to the active screen for keyboard input.
    pub fn scroll_to_bottom(&mut self) {
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
    }

    /// Returns the raw viewport position, height and total number of scrollable rows.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot read the scroll state.
    pub fn scrollbar(&self) -> Result<Scrollbar, VtError> {
        self.terminal.scrollbar()
    }

    /// Returns the cursor's row within the active screen, including when the cursor is hidden.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot read the cursor position.
    pub fn cursor_row(&self) -> Result<u16, VtError> {
        self.terminal.cursor_y()
    }

    /// Encodes a key using the running program's active keyboard modes.
    ///
    /// # Errors
    /// Returns Ghostty's error if encoding fails.
    pub fn encode_key(
        &mut self,
        key: key::Key,
        action: key::Action,
        mods: key::Mods,
        consumed: key::Mods,
        base: Option<char>,
        text: Option<&str>,
    ) -> Result<Vec<u8>, VtError> {
        self.event
            .set_key(key)
            .set_action(action)
            .set_mods(mods)
            .set_consumed_mods(consumed)
            .set_unshifted_codepoint(base.unwrap_or('\0'))
            .set_utf8(text);
        let mut bytes = Vec::new();
        self.encoder
            .set_options_from_terminal(&self.terminal)
            .encode_to_vec(&self.event, &mut bytes)?;
        Ok(bytes)
    }

    /// Encodes clipboard text using the active bracketed-paste mode.
    ///
    /// # Errors
    /// Returns Ghostty's error if encoding fails.
    pub fn encode_paste(&self, text: &str) -> Result<Vec<u8>, VtError> {
        let mut data = text.as_bytes().to_vec();
        let mut bytes = vec![0; data.len() + 12];
        let length = paste::encode(
            &mut data,
            self.terminal.mode(Mode::BRACKETED_PASTE)?,
            &mut bytes,
        )?;
        bytes.truncate(length);
        Ok(bytes)
    }

    /// Encodes a focus transition when the running program has enabled focus reports.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot read the mode or encode the event.
    pub fn encode_focus(&self, focused: bool) -> Result<Vec<u8>, VtError> {
        if !self.terminal.mode(Mode::FOCUS_EVENT)? {
            return Ok(Vec::new());
        }
        let event = if focused {
            focus::Event::Gained
        } else {
            focus::Event::Lost
        };
        let mut bytes = [0; 8];
        let length = event.encode(&mut bytes)?;
        Ok(bytes.into_iter().take(length).collect())
    }

    /// Updates the cached screen and returns whether anything visible changed.
    ///
    /// # Errors
    /// Returns Ghostty's error if it cannot read the render state.
    pub fn update(&mut self) -> Result<bool, VtError> {
        self.renderer.update(&self.terminal)
    }

    /// The last updated viewport.
    pub fn screen(&self) -> &Screen {
        &self.renderer.screen
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "Fail the test when Ghostty rejects the fixture"
)]
mod tests {
    use super::*;

    fn emulator() -> Emulator {
        Emulator::new(Size {
            cols: 20,
            rows: 3,
            cell_width: 7,
            cell_height: 20,
        })
        .unwrap()
    }

    #[test]
    fn updates_damaged_rows_and_tracks_cursor() {
        let mut emulator = emulator();
        emulator.feed("hello\r\n\x1b[31m界!".as_bytes());
        assert!(emulator.update().unwrap());
        let screen = emulator.screen();
        let hello = screen.rows[0].as_ref().unwrap();
        let world = screen.rows[1].as_ref().unwrap();
        assert_eq!(
            hello
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            "hello"
        );
        assert_eq!((&*world.cells[0].text, world.cells[0].width), ("界", 2));
        assert!(world.cells[0].foreground.is_some());
        assert_eq!(world.cells[1].column, 2);
        assert_eq!(screen.cursor.as_ref().unwrap().column, 3);
        let revision = hello.revision;
        assert!(!emulator.update().unwrap());
        emulator.feed(b"?");
        assert!(emulator.update().unwrap());
        assert_eq!(
            emulator.screen().rows[0].as_ref().unwrap().revision,
            revision
        );
    }

    #[test]
    fn encodes_control_keys_and_bracketed_paste() {
        let mut emulator = emulator();
        let bytes = emulator
            .encode_key(
                key::Key::C,
                key::Action::Press,
                key::Mods::CTRL,
                key::Mods::empty(),
                Some('c'),
                None,
            )
            .unwrap();
        assert_eq!(bytes, b"\x03");
        emulator.feed(b"\x1b[?2004h");
        assert_eq!(
            emulator.encode_paste("hello").unwrap(),
            b"\x1b[200~hello\x1b[201~"
        );
    }

    #[test]
    fn copies_hard_lines_and_joins_soft_wraps() -> Result<(), VtError> {
        let mut emulator = Emulator::new(Size {
            cols: 20,
            rows: 5,
            cell_width: 7,
            cell_height: 20,
        })?;
        emulator.feed("  alpha   \r\nabcdefghijklmnopqrs tuvw\r\n界!   ".as_bytes());
        emulator.update()?;
        emulator.start_selection([0.1, 0.0], 1)?;
        assert_eq!(emulator.selected_text()?, None);
        emulator.drag_selection([19.9, 3.0])?;
        emulator.finish_selection()?;
        assert_eq!(
            emulator.selected_text()?.as_deref(),
            Some("  alpha\nabcdefghijklmnopqrs tuvw\n界!")
        );
        assert!(emulator.update()?);
        assert_eq!(
            emulator.screen().selection,
            [Some(0..=19), Some(0..=19), Some(0..=19), Some(0..=19), None]
        );
        emulator.start_selection([3.0, 0.0], 1)?;
        emulator.finish_selection()?;
        assert_eq!(emulator.selected_text()?, None);
        assert!(emulator.update()?);
        assert!(emulator.screen().selection.iter().all(Option::is_none));
        Ok(())
    }

    #[test]
    fn selects_words_and_lines_and_keeps_them_anchored_in_history() -> Result<(), VtError> {
        let mut emulator = emulator();
        emulator.feed(b"first word\r\nsecond\r\nthird");
        emulator.update()?;
        emulator.start_selection([7.0, 0.0], 2)?;
        emulator.finish_selection()?;
        assert_eq!(emulator.selected_text()?.as_deref(), Some("word"));
        emulator.start_selection([7.0, 0.0], 3)?;
        emulator.finish_selection()?;
        assert_eq!(emulator.selected_text()?.as_deref(), Some("first word"));

        emulator.feed(b"\r\nfourth");
        assert!(emulator.update()?);
        assert!(emulator.screen().selection.iter().all(Option::is_none));
        assert_eq!(emulator.selected_text()?.as_deref(), Some("first word"));
        emulator.scroll_to_row(0);
        assert!(emulator.update()?);
        assert_eq!(emulator.screen().selection.first(), Some(&Some(0..=9)));

        emulator.scroll_to_bottom();
        emulator.start_selection([1.9, 0.0], 1)?;
        emulator.scroll_to_row(0);
        emulator.drag_selection([0.1, 0.0])?;
        emulator.finish_selection()?;
        assert_eq!(emulator.selected_text()?.as_deref(), Some("first word\nse"));
        Ok(())
    }

    #[test]
    fn copy_preserves_selected_blank_rows() -> Result<(), VtError> {
        let mut emulator = emulator();
        emulator.feed("one\r\n    \r\ntwo\u{2003} ".as_bytes());
        emulator.start_selection([0.1, 0.0], 1)?;
        emulator.drag_selection([19.9, 2.0])?;
        assert_eq!(emulator.selected_text()?.as_deref(), Some("one\n\ntwo"));
        emulator.clear_selection()?;

        emulator.feed(b"\r\n    ");
        emulator.start_selection([0.1, 1.0], 1)?;
        emulator.drag_selection([19.9, 2.0])?;
        assert_eq!(emulator.selected_text()?.as_deref(), Some("two\n"));
        Ok(())
    }

    #[test]
    fn selects_live_and_history_rows_from_a_rex_snapshot() -> Result<(), VtError> {
        let mut original = emulator();
        original.feed(b"history\r\nactive\r\nlast\r\n");
        let bytes = snapshot::encode(&original.terminal)?;
        let mut restored = Emulator::from_snapshot(&bytes)?;
        restored.start_selection([0.1, 0.0], 2)?;
        assert_eq!(restored.selected_text()?.as_deref(), Some("active"));
        restored.finish_selection()?;
        restored.scroll_to_row(0);
        restored.start_selection([0.1, 0.0], 1)?;
        restored.drag_selection([19.9, 1.0])?;
        assert_eq!(
            restored.selected_text()?.as_deref(),
            Some("history\nactive")
        );
        Ok(())
    }

    #[test]
    fn encodes_mouse_press_drag_release_and_wheel_in_program_mode() -> Result<(), VtError> {
        use ghostty_vt::mouse::{Action, Button, Position};

        let mut emulator = emulator();
        assert!(!emulator.is_mouse_tracking());
        emulator.feed(b"\x1b[?1002h\x1b[?1006h");
        assert!(emulator.is_mouse_tracking());
        for (action, button, x, expected) in [
            (Action::Press, Button::Left, 1.5, b"\x1b[<0;2;3M".as_slice()),
            (
                Action::Motion,
                Button::Left,
                2.25,
                b"\x1b[<32;3;3M".as_slice(),
            ),
            (
                Action::Release,
                Button::Left,
                2.25,
                b"\x1b[<0;3;3m".as_slice(),
            ),
            (
                Action::Press,
                Button::Four,
                1.5,
                b"\x1b[<64;2;3M".as_slice(),
            ),
        ] {
            assert_eq!(
                emulator.encode_mouse(
                    action,
                    Some(button),
                    key::Mods::empty(),
                    Position { x, y: 2.25 },
                )?,
                expected
            );
        }
        Ok(())
    }
}
