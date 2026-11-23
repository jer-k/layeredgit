use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use rune::{Color, Ime, KeyEvent, Tx, Waker, color};
use rune_kit::{
    Kit,
    syntax::{Syntax, SyntaxTheme},
};
use term::{Emulator, LocalPty, RgbColor, Screen, Size, Source, SourceEvent, key};

use super::State;

mod keys;

pub(crate) const SCREEN_KEY: &str = "terminal-screen";
pub(crate) const CELL_HEIGHT: f32 = 20.0;
pub(crate) const CELL_WIDTH: f32 = 7.25;
const TOGGLE: rune::Chord = rune::Chord::new(rune::ChordKey::Char('`')).with(rune::ChordMod::Ctrl);
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(crate) struct TerminalPane {
    pub(crate) open: bool,
    pub(crate) focused: bool,
    pub(super) window_blurred: bool,
    pub(crate) preedit: String,
    pub(crate) id: u64,
    pub(crate) version: u64,
    geometry: Option<(Size, bool)>,
    session: Option<Session>,
    status: Status,
    wheel_remainder: f32,
}

#[derive(Default)]
enum Status {
    #[default]
    Starting,
    Running,
    Ended(String),
}

struct Session {
    emulator: Emulator,
    source: Option<Box<dyn Source>>,
}

impl TerminalPane {
    pub(crate) fn status(&self) -> Option<&str> {
        match &self.status {
            Status::Starting => Some("Starting shell…"),
            Status::Running => None,
            Status::Ended(message) => Some(message),
        }
    }

    pub(crate) fn ended(&self) -> bool {
        matches!(self.status, Status::Ended(_))
    }

    pub(crate) fn screen(&self) -> Option<&Screen> {
        self.session
            .as_ref()
            .map(|session| session.emulator.screen())
    }

    fn start(&mut self, cwd: PathBuf, waker: Waker) {
        let Some((size, dark)) = self.geometry else {
            return;
        };
        self.id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        self.wheel_remainder = 0.0;
        self.preedit.clear();
        self.status = Status::Starting;
        self.session = None;
        let result = (|| -> Result<Session, String> {
            let mut emulator = Emulator::new(size).map_err(|error| error.to_string())?;
            apply_theme(&mut emulator, dark).map_err(|error| error.to_string())?;
            emulator.update().map_err(|error| error.to_string())?;
            let source =
                LocalPty::login_shell(cwd, size, waker).map_err(|error| error.to_string())?;
            Ok(Session {
                emulator,
                source: Some(Box::new(source)),
            })
        })();
        match result {
            Ok(session) => self.session = Some(session),
            Err(error) => self.fail(&error),
        }
    }

    fn fail(&mut self, error: &str) {
        self.status = Status::Ended(format!("Terminal error · {error}"));
        if let Some(session) = &mut self.session {
            session.source = None;
        }
    }

    fn refresh_screen(&mut self) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        match session.emulator.update() {
            Ok(false) => false,
            Ok(true) => {
                self.version = self.version.wrapping_add(1);
                true
            }
            Err(error) => {
                self.fail(&error.to_string());
                true
            }
        }
    }

    fn configure(&mut self, size: Size, dark: bool) -> bool {
        let previous = self.geometry.replace((size, dark));
        if previous == Some((size, dark)) {
            return false;
        }
        let Some(session) = &mut self.session else {
            return false;
        };
        let result = (|| {
            if previous.is_none_or(|(_, was_dark)| was_dark != dark) {
                apply_theme(&mut session.emulator, dark)?;
            }
            if previous.is_none_or(|(previous, _)| previous != size) {
                session.emulator.resize(size)?;
                if let Some(source) = &session.source {
                    source.resize(size);
                    session.emulator.flush_responses(source.as_ref());
                }
            }
            Ok::<_, term::VtError>(())
        })();
        if let Err(error) = result {
            self.fail(&error.to_string());
            return true;
        }
        self.refresh_screen()
    }

    fn poll_output(&mut self, tx: &mut Tx) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        let Some(source) = &session.source else {
            return false;
        };
        let mut received = false;
        let mut status_changed = false;
        let mut ended = None;
        // Bounded work per frame so a busy terminal leaves time for input and layout.
        for index in 0..32 {
            match source.try_event() {
                Ok(SourceEvent::Started) => {
                    self.status = Status::Running;
                    status_changed = true;
                }
                Ok(SourceEvent::Output(bytes)) => {
                    session.emulator.feed(&bytes);
                    session.emulator.flush_responses(source.as_ref());
                    received = true;
                }
                Ok(SourceEvent::Exited(status)) => {
                    ended = Some(format!("Shell exited · {status}"));
                    break;
                }
                Ok(SourceEvent::Failed(error)) => {
                    ended = Some(format!("Terminal error · {error}"));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    ended = Some("Shell disconnected".into());
                    break;
                }
            }
            if index == 31 {
                tx.wake_at(tx.clock());
            }
        }
        if let Some(message) = ended {
            self.status = Status::Ended(message);
            session.source = None;
            self.preedit.clear();
            status_changed = true;
        }
        let screen_changed = received && self.refresh_screen();
        status_changed || screen_changed
    }

    pub(super) fn set_focus(&mut self, focused: bool) -> bool {
        if std::mem::replace(&mut self.focused, focused) == focused {
            return false;
        }
        self.preedit.clear();
        if let Some(session) = &self.session
            && let Some(source) = &session.source
            && let Ok(bytes) = session.emulator.encode_focus(focused)
        {
            source.write(bytes);
        }
        true
    }

    fn input(
        &mut self,
        encode: impl FnOnce(&mut Emulator) -> Result<Vec<u8>, term::VtError>,
    ) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        let Some(source) = &session.source else {
            return false;
        };
        match encode(&mut session.emulator) {
            Ok(bytes) if bytes.is_empty() => false,
            Ok(bytes) => {
                source.write(bytes);
                session.emulator.scroll_to_bottom();
                self.refresh_screen()
            }
            Err(error) => {
                self.fail(&error.to_string());
                true
            }
        }
    }
}

impl State {
    pub(crate) fn terminal_shortcut(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if TOGGLE.matches_tx(event, tx) && self.terminal_target().is_some() {
            if !tx.key_repeat() {
                self.toggle_terminal(tx);
            }
            true
        } else {
            false
        }
    }

    pub(crate) fn toggle_terminal(&mut self, tx: &mut Tx) {
        if self.terminal.open && !self.terminal.ended() {
            self.hide_terminal(tx);
            return;
        }
        let Some((cwd, waker)) = self.terminal_target() else {
            return;
        };
        self.terminal.open = true;
        if self.terminal.session.is_none() || self.terminal.ended() {
            self.terminal.start(cwd, waker);
        }
        tx.focus_when_shown(SCREEN_KEY, false);
        tx.instant();
    }

    pub(crate) fn hide_terminal(&mut self, tx: &mut Tx) {
        self.terminal.open = false;
        self.terminal.set_focus(false);
        tx.instant();
    }

    pub(crate) fn layout_terminal(&mut self, size: Size, dark: bool, tx: &mut Tx) {
        let changed = self.terminal.configure(size, dark);
        if self.terminal.open
            && self.terminal.session.is_none()
            && !self.terminal.ended()
            && let Some((cwd, waker)) = self.terminal_target()
        {
            self.terminal.start(cwd, waker);
            tx.instant();
        } else if changed {
            tx.instant();
        } else {
            tx.live_only();
        }
    }

    pub(crate) fn terminal_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if self.terminal_shortcut(event, tx) {
            return true;
        }
        // Command shortcuts still belong to the application.
        if event.mods.logo || (event.mods.cmd && !event.mods.ctrl) {
            return false;
        }
        let action = if tx.key_repeat() {
            key::Action::Repeat
        } else {
            key::Action::Press
        };
        if !self
            .terminal
            .input(|emulator| keys::encode(emulator, event, tx, action))
        {
            tx.live_only();
        }
        true
    }

    pub(crate) fn terminal_key_up(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if tx.focused() != Some(SCREEN_KEY) || event.mods.logo || TOGGLE.matches_tx(event, tx) {
            return false;
        }
        if !self
            .terminal
            .input(|emulator| keys::encode(emulator, event, tx, key::Action::Release))
        {
            tx.live_only();
        }
        true
    }

    pub(crate) fn terminal_paste(&mut self, text: &str, tx: &mut Tx) {
        if tx.focused() != Some(SCREEN_KEY)
            || !self.terminal.input(|emulator| emulator.encode_paste(text))
        {
            tx.live_only();
        }
    }

    pub(crate) fn terminal_ime(&mut self, ime: &Ime, tx: &mut Tx) -> bool {
        if tx.focused() != Some(SCREEN_KEY) {
            self.terminal.preedit.clear();
            return false;
        }
        match ime {
            Ime::Commit(text) => {
                self.terminal.preedit.clear();
                self.terminal.input(|emulator| {
                    emulator.encode_key(
                        key::Key::Unidentified,
                        key::Action::Press,
                        key::Mods::empty(),
                        key::Mods::empty(),
                        None,
                        Some(text),
                    )
                });
            }
            Ime::Preedit(text, ..) => self.terminal.preedit.clone_from(text),
            _ => return false,
        }
        true
    }

    pub(crate) fn terminal_window_focus(&mut self, focused: bool, tx: &mut Tx) {
        self.terminal.window_blurred = !focused;
        if !self
            .terminal
            .set_focus(focused && self.terminal.open && tx.focused() == Some(SCREEN_KEY))
        {
            tx.live_only();
        }
    }

    pub(crate) fn terminal_scroll(&mut self, dy: f32, tx: &mut Tx) -> bool {
        if let Some(session) = &mut self.terminal.session {
            self.terminal.wheel_remainder += dy / CELL_HEIGHT;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Keep only whole rows; retain the remainder"
            )]
            let rows = self.terminal.wheel_remainder.trunc().clamp(-1000.0, 1000.0) as i16;
            if rows != 0 {
                self.terminal.wheel_remainder -= f32::from(rows);
                session.emulator.scroll(isize::from(rows));
                if self.terminal.refresh_screen() {
                    return true;
                }
            }
        }
        tx.live_only();
        true
    }

    pub(super) fn poll_terminal(&mut self, tx: &mut Tx) -> bool {
        if let Some(workspace) = &mut self.workspace {
            for document in workspace
                .entries
                .iter_mut()
                .filter_map(|entry| entry.document.as_mut())
            {
                document.terminal.poll_output(tx);
            }
        }
        let changed = self.terminal.poll_output(tx);
        let focused = self.terminal.set_focus(
            self.terminal.open && !self.terminal.window_blurred && tx.focused() == Some(SCREEN_KEY),
        );
        self.terminal.open && (changed || focused)
    }

    fn terminal_target(&self) -> Option<(PathBuf, Waker)> {
        let workspace = self.workspace.as_ref()?;
        Some((
            workspace.active()?.repository.path.clone(),
            workspace.waker.clone()?,
        ))
    }
}

fn apply_theme(emulator: &mut Emulator, dark: bool) -> Result<(), term::VtError> {
    let kit = Kit::neutral(dark);
    let c = &kit.c;
    let syntax = SyntaxTheme::from_kit(&kit);
    let black = Color::BLACK.mix(c.bg_base, if dark { 0.65 } else { 0.12 });
    let white = Color::WHITE.mix(c.text_base, if dark { 0.12 } else { 0.68 });
    let chromatic = [
        c.danger.fg,
        c.success.fg,
        c.warning.fg,
        syntax.color(Syntax::Function),
        syntax.color(Syntax::Keyword),
        syntax.color(Syntax::Type),
    ];
    let [red, green, yellow, blue, magenta, cyan] = chromatic;
    let [
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
    ] = chromatic.map(|color| color.mix(c.text_base, 0.26));
    let colors = [
        black,
        red,
        green,
        yellow,
        blue,
        magenta,
        cyan,
        white,
        c.text_muted,
        bright_red,
        bright_green,
        bright_yellow,
        bright_blue,
        bright_magenta,
        bright_cyan,
        if dark { Color::WHITE } else { c.text_muted },
    ];
    emulator.theme(dark, rgb(c.text_base), rgb(c.bg_base), colors.map(rgb))
}

fn rgb(color: Color) -> RgbColor {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Convert bounded sRGB channels to bytes"
    )]
    let [r, g, b] = [color.r, color.g, color.b]
        .map(|c| (color::srgb_encode(c.clamp(0.0, 1.0)) * 255.0).round() as u8);
    RgbColor { r, g, b }
}
