use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::TryRecvError,
    },
};

use rune::{Color, Ime, KeyEvent, Tx, Waker, color};
use rune_kit::{
    Kit,
    syntax::{Syntax, SyntaxTheme},
};
use term::{Emulator, LocalPty, RexPty, RgbColor, Screen, Size, Source, SourceEvent, key};

use super::State;

mod keys;
mod viewport;

use viewport::{Grid, Viewport};

pub(crate) const SCREEN_KEY: &str = "terminal-screen";
pub(crate) const CELL_HEIGHT: f32 = 20.0;
pub(crate) const CELL_WIDTH: f32 = 7.25;
const TOGGLE: rune::Chord = rune::Chord::new(rune::ChordKey::Char('`')).with(rune::ChordMod::Ctrl);
const OPEN: rune::Chord = rune::Chord::new(rune::ChordKey::Char('t')).with(rune::ChordMod::Super);
static NEXT_EMULATOR: AtomicU64 = AtomicU64::new(1);

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
    notice: Option<String>,
    wheel_remainder: f32,
    viewport: Viewport,
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
    source: Option<Transport>,
    backend: Backend,
}

enum Backend {
    Connecting { cwd: PathBuf, waker: Waker },
    Local,
    Rex(String),
}

enum Transport {
    Local(LocalPty),
    Rex(RexPty),
}

impl Transport {
    fn source(&self) -> &dyn Source {
        match self {
            Self::Local(source) => source,
            Self::Rex(source) => source,
        }
    }

    fn drain_replies(&self, emulator: &mut Emulator) {
        match self {
            Self::Local(source) => emulator.flush_responses(source),
            // Rex's emulator is already responsible for answering terminal queries.
            Self::Rex(_) => emulator.discard_responses(),
        }
    }
}

impl Session {
    fn follows_remote_size(&self) -> bool {
        matches!(self.backend, Backend::Rex(_))
    }

    fn viewport_grid(&self) -> Result<Grid, term::VtError> {
        let scrollbar = self.emulator.scrollbar()?;
        let row_count = |rows| usize::try_from(rows).unwrap_or(usize::MAX);
        Ok(Grid {
            rows: row_count(scrollbar.len),
            offset: row_count(scrollbar.offset),
            history: row_count(scrollbar.total.saturating_sub(scrollbar.len)),
            cursor: usize::from(self.emulator.cursor_row()?),
        })
    }

    fn report_focus(&self, focused: bool) {
        match &self.source {
            Some(Transport::Rex(source)) => source.focus(focused),
            Some(Transport::Local(source)) => {
                if let Ok(bytes) = self.emulator.encode_focus(focused) {
                    source.write(bytes);
                }
            }
            None => {}
        }
    }
}

impl TerminalPane {
    pub(crate) fn status(&self) -> Option<&str> {
        match &self.status {
            Status::Starting => Some("Starting shell…"),
            Status::Running => self.notice.as_deref(),
            Status::Ended(message) => Some(message),
        }
    }

    pub(crate) fn backend_label(&self) -> Option<&str> {
        match &self.session.as_ref()?.backend {
            Backend::Connecting { .. } => None,
            Backend::Local => Some("Local shell"),
            Backend::Rex(label) => Some(label),
        }
    }

    pub(crate) fn can_open_in_rex(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session.follows_remote_size() && matches!(session.source, Some(Transport::Rex(_)))
        })
    }

    pub(crate) fn ended(&self) -> bool {
        matches!(self.status, Status::Ended(_))
    }

    pub(crate) fn screen(&self) -> Option<&Screen> {
        self.session
            .as_ref()
            .map(|session| session.emulator.screen())
    }

    pub(crate) fn visible_rows(&self) -> Range<usize> {
        self.viewport.visible(
            self.screen().map_or(0, |screen| screen.rows.len()),
            self.pane_rows(),
        )
    }

    pub(crate) fn columns(&self) -> u16 {
        self.geometry.map_or(0, |(size, _)| size.cols)
    }

    fn pane_rows(&self) -> usize {
        self.geometry.map_or(0, |(size, _)| usize::from(size.rows))
    }

    fn start(&mut self, cwd: PathBuf, waker: Waker) {
        let Some((size, dark)) = self.geometry else {
            return;
        };
        self.id = NEXT_EMULATOR.fetch_add(1, Ordering::Relaxed);
        self.wheel_remainder = 0.0;
        self.viewport = Viewport::default();
        self.preedit.clear();
        self.status = Status::Starting;
        self.notice = None;
        self.session = None;
        let result = (|| -> Result<Session, String> {
            let mut emulator = Emulator::new(size).map_err(|error| error.to_string())?;
            apply_theme(&mut emulator, dark).map_err(|error| error.to_string())?;
            emulator.update().map_err(|error| error.to_string())?;
            let source = RexPty::connect(cwd.clone(), size, waker.clone())
                .ok()
                .map(Transport::Rex);
            Ok(Session {
                emulator,
                source,
                backend: Backend::Connecting { cwd, waker },
            })
        })();
        match result {
            Ok(session) => {
                let use_local = session.source.is_none();
                if self.focused {
                    session.report_focus(true);
                }
                self.session = Some(session);
                if use_local {
                    self.start_local();
                }
            }
            Err(error) => self.fail(&error),
        }
    }

    fn start_local(&mut self) {
        let Some((size, _)) = self.geometry else {
            return;
        };
        let Some(session) = &mut self.session else {
            return;
        };
        let Backend::Connecting { cwd, waker } =
            std::mem::replace(&mut session.backend, Backend::Local)
        else {
            return;
        };
        match LocalPty::login_shell(cwd, size, waker) {
            Ok(source) => session.source = Some(Transport::Local(source)),
            Err(error) => self.fail(&error.to_string()),
        }
    }

    fn fail(&mut self, error: &str) {
        self.status = Status::Ended(format!("Terminal error · {error}"));
        if let Some(session) = &mut self.session {
            session.source = None;
        }
    }

    fn refresh_screen(&mut self) -> bool {
        let pane_rows = self.pane_rows();
        let Some(session) = &mut self.session else {
            return false;
        };
        let result = (|| {
            let first = self.viewport.first;
            if let Some(offset) = self.viewport.layout(&session.viewport_grid()?, pane_rows) {
                session.emulator.scroll_to_row(offset);
            }
            Ok::<_, term::VtError>(session.emulator.update()? || first != self.viewport.first)
        })();
        match result {
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
                if !session.follows_remote_size() {
                    session.emulator.resize(size)?;
                }
                if let Some(source) = &session.source {
                    source.source().resize(size);
                    source.drain_replies(&mut session.emulator);
                }
            }
            Ok::<_, term::VtError>(())
        })();
        if let Err(error) = result {
            self.fail(&error.to_string());
            return true;
        }
        self.refresh_screen() || previous.is_none_or(|(previous, _)| previous != size)
    }

    fn poll_output(&mut self, tx: &mut Tx) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        let Some(source) = &session.source else {
            return false;
        };
        let geometry = self.geometry;
        let mut received = false;
        let mut status_changed = false;
        let mut ended = None;
        // Bounded work per frame so a busy terminal leaves time for input and layout.
        for index in 0..32 {
            match source.source().try_event() {
                Ok(SourceEvent::Started) => {
                    self.status = Status::Running;
                    status_changed = true;
                }
                Ok(SourceEvent::RexAttached {
                    session_label,
                    snapshot,
                }) => {
                    let dark = geometry.is_some_and(|(_, dark)| dark);
                    let Ok(emulator) =
                        Emulator::from_snapshot(&snapshot).and_then(|mut emulator| {
                            apply_theme(&mut emulator, dark)?;
                            Ok(emulator)
                        })
                    else {
                        self.start_local();
                        return true;
                    };
                    // The snapshot can reuse row IDs and revisions from the blank emulator.
                    self.id = NEXT_EMULATOR.fetch_add(1, Ordering::Relaxed);
                    session.emulator = emulator;
                    session.backend = Backend::Rex(format!("Rex · {session_label}"));
                    self.status = Status::Running;
                    status_changed = true;
                    received = true;
                }
                Ok(SourceEvent::Resized(mut size)) => {
                    if let Some((pane, _)) = geometry
                        && (size.cell_width == 0 || size.cell_height == 0)
                    {
                        size.cell_width = pane.cell_width;
                        size.cell_height = pane.cell_height;
                    }
                    if let Err(error) = session.emulator.resize(size) {
                        ended = Some(format!("Terminal error · {error}"));
                        break;
                    }
                    source.drain_replies(&mut session.emulator);
                    received = true;
                }
                Ok(SourceEvent::Notice(message)) => {
                    self.notice = Some(message);
                    status_changed = true;
                }
                Ok(SourceEvent::Output(bytes)) => {
                    session.emulator.feed(&bytes);
                    source.drain_replies(&mut session.emulator);
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
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    ended = Some("Shell disconnected".into());
                    break;
                }
            }
            if index == 31 {
                tx.wake_at(tx.clock());
            }
        }
        if let Some(message) = ended {
            if matches!(session.backend, Backend::Connecting { .. }) {
                self.start_local();
                return true;
            }
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
        if let Some(session) = &self.session {
            session.report_focus(focused);
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
                source.source().write(bytes);
                self.viewport.follow();
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
        } else if OPEN.matches_tx(event, tx)
            && (!self.terminal.open || tx.focused() != Some(SCREEN_KEY))
            && let Some((cwd, waker)) = self.terminal_target()
        {
            if !tx.key_repeat() {
                self.show_terminal(cwd, waker, tx);
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
        self.show_terminal(cwd, waker, tx);
    }

    fn show_terminal(&mut self, cwd: PathBuf, waker: Waker, tx: &mut Tx) {
        self.terminal.open = true;
        if self.terminal.session.is_none() || self.terminal.ended() {
            self.terminal.start(cwd, waker);
        }
        tx.focus_when_shown(SCREEN_KEY, false);
        tx.instant();
    }

    pub(crate) fn open_terminal_in_rex(&mut self, tx: &mut Tx) {
        if let Some(Transport::Rex(source)) = self
            .terminal
            .session
            .as_ref()
            .and_then(|session| session.source.as_ref())
        {
            source.open_in_rex();
            self.terminal.notice = None;
            tx.instant();
        }
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
        if keys::super_held(event) && !OPEN.matches_tx(event, tx) {
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
        if tx.focused() != Some(SCREEN_KEY)
            || (keys::super_held(event) && !OPEN.matches_tx(event, tx))
            || TOGGLE.matches_tx(event, tx)
        {
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
        let pane_rows = self.terminal.pane_rows();
        if let Some(session) = &mut self.terminal.session {
            // Rune's dy is positive when content should move down, which reveals older rows.
            self.terminal.wheel_remainder -= dy / CELL_HEIGHT;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "Keep only whole rows; retain the remainder"
            )]
            let rows = self.terminal.wheel_remainder.trunc().clamp(-1000.0, 1000.0) as i16;
            if rows != 0 {
                self.terminal.wheel_remainder -= f32::from(rows);
                let grid = match session.viewport_grid() {
                    Ok(grid) => grid,
                    Err(error) => {
                        self.terminal.fail(&error.to_string());
                        return true;
                    }
                };
                if let Some(offset) =
                    self.terminal
                        .viewport
                        .scroll(&grid, pane_rows, isize::from(rows))
                {
                    session.emulator.scroll_to_row(offset);
                }
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
