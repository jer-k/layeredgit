//! Review interactions and the current virtual list projection.

mod files;
mod pulse;
mod restore;

pub(crate) use files::FileTree;
use files::Sidebar;
pub(crate) use pulse::PulseDirection;
#[cfg(test)]
mod tests;

use std::{cell::Cell, collections::HashSet, ffi::OsString, ops::Range, sync::Arc};

use rune::{Key, KeyEvent, Tx, Waker};

use crate::{
    desktop::{ReviewSession, open_shortcut},
    model::{Filter, Review, ReviewRows, RowKind, fresh::Replacement},
    palette::Palette,
    source::{Refresh, Repository},
};

pub(crate) const HEADER_HEIGHT: f32 = 44.0;

/// Loading and error messages for the initial review or a live refresh.
pub(crate) enum Status {
    Loading,
    Failed(String),
    Ready { refresh_error: Option<String> },
}

pub(crate) struct State {
    pub(crate) session: Option<ReviewSession>,
    pub(crate) review: Arc<Review>,
    pub(crate) status: Status,
    refresh: Option<Refresh>,
    demo_reload: Option<Replacement>,
    pub(crate) headless_reduced_motion: bool,
    pub(crate) pulse: pulse::Pulse,
    system_dark: Cell<bool>,
    #[cfg(feature = "dev")]
    pub(crate) tuning: crate::palette::Tuning,
    pub(crate) filter: Filter,
    pub(crate) rows: ReviewRows,
    pub(crate) rows_version: u64,
    pub(crate) review_version: u64,
    pub(crate) wrap: bool,
    sidebar: Sidebar,
    pub(crate) active_file: Option<usize>,
    pub(crate) sticky_file: Option<usize>,
    pub(crate) collapsed: HashSet<usize>,
    expanded: HashSet<(usize, usize, usize)>,
    visible_rows: Range<usize>,
    navigation: Option<usize>,
    sidebar_reveal: Option<usize>,
}

impl State {
    pub(crate) fn open(repository: Option<Repository>, headless: bool, demo_pulse: bool) -> Self {
        let input = match repository {
            None => Ok(crate::demo::review()),
            Some(repository) if headless => repository.load_input(),
            Some(repository) => {
                return Self::new(
                    Review::default(),
                    Status::Loading,
                    Some(Refresh::new(repository)),
                );
            }
        };
        match input {
            Ok(input) => {
                let updated = (headless && demo_pulse).then(|| crate::demo::edited(input.clone()));
                let mut state = Self::new(Review::new(input), ready(), None);
                state.demo_reload =
                    updated.map(|input| Replacement::new(Review::new(input), &state.review));
                state
            }
            Err(error) => Self::new(Review::default(), Status::Failed(error.to_string()), None),
        }
    }

    fn new(review: Review, status: Status, refresh: Option<Refresh>) -> Self {
        let filter = Filter::All;
        let collapsed = HashSet::new();
        let expanded = HashSet::new();
        let rows = ReviewRows::new(&review, filter, &collapsed, &expanded);
        let active_file = rows.files.first().copied();
        let mut sidebar = Sidebar::new(&review, filter);
        if let Some(file) = active_file {
            sidebar.select(file);
        }
        Self {
            session: None,
            review: Arc::new(review),
            status,
            refresh,
            demo_reload: None,
            headless_reduced_motion: false,
            pulse: pulse::Pulse::default(),
            system_dark: Cell::default(),
            #[cfg(feature = "dev")]
            tuning: crate::palette::Tuning::default(),
            filter,
            rows,
            rows_version: 0,
            review_version: 0,
            wrap: false,
            sidebar,
            active_file,
            sticky_file: None,
            collapsed,
            expanded,
            visible_rows: 0..0,
            navigation: None,
            sidebar_reveal: None,
        }
    }

    pub(crate) fn can_refresh(&self) -> bool {
        self.refresh.is_some()
    }

    pub(crate) fn force_reduced_motion(&mut self) {
        self.headless_reduced_motion = true;
    }

    /// Shares the current system appearance with the pulse timer.
    pub(crate) fn palette(&self, dark: bool) -> Palette {
        self.system_dark.set(dark);
        #[cfg(feature = "dev")]
        return self.tuning.palette(dark);
        #[cfg(not(feature = "dev"))]
        Palette::defaults(dark)
    }

    pub(crate) fn setup(&mut self, waker: Waker) {
        if let Some(refresh) = &mut self.refresh {
            refresh.setup(waker);
        }
    }

    pub(crate) fn visibility(&mut self, visible: bool, tx: &mut Tx) {
        if let Some(refresh) = &mut self.refresh {
            refresh.visibility(visible, tx);
        }
        tx.live_only();
    }

    pub(crate) fn request_refresh(&mut self, tx: &mut Tx) -> bool {
        if let Some(refresh) = &mut self.refresh {
            refresh.force(tx);
            tx.live_only();
            true
        } else {
            false
        }
    }

    pub(crate) fn title(&mut self, tx: &mut Tx) {
        if let Some(session) = &mut self.session {
            session.set_title(None, tx);
        }
    }

    pub(crate) fn set_base(&mut self, base: OsString, tx: &mut Tx) {
        if let Some(refresh) = &mut self.refresh {
            refresh.set_base(base, tx);
        }
    }

    pub(crate) fn close(&mut self, _: &mut Tx) {
        self.refresh = None;
        if let Some(session) = &self.session {
            session.close();
        }
    }

    pub(crate) fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        self.reveal_sidebar(tx);
        let pulse_ended = self.pulse.files().next().is_some() && {
            let seconds = self.palette(self.system_dark.get()).pulse_seconds;
            self.pulse.poll(now, seconds, tx)
        };
        if self.demo_reload.is_some() {
            if now < 0.5 {
                tx.wake_at(0.5);
            } else if let Some(update) = self.demo_reload.take() {
                self.replace_review(update, tx);
                return true;
            }
        }
        let reloaded = match self
            .refresh
            .as_mut()
            .and_then(|refresh| refresh.poll(now, &self.review, tx))
        {
            Some(Ok(review)) => {
                self.replace_review(review, tx);
                true
            }
            Some(Err(error)) => self.show_error(error.to_string()),
            None => false,
        };
        pulse_ended || reloaded
    }

    fn replace_review(&mut self, update: Replacement, tx: &mut Tx) {
        let Replacement {
            review,
            files,
            fresh,
            same_comparison,
        } = update;
        let old = std::mem::replace(&mut self.review, review);
        if let Some(session) = &mut self.session {
            session.set_title(Some(&self.review.head_label), tx);
        }
        let loaded = matches!(self.status, Status::Ready { .. });
        self.status = ready();
        self.review_version = self.review_version.wrapping_add(1);
        self.sidebar.rebuild(&self.review, self.filter);
        if !loaded {
            self.rebuild_rows(tx);
            self.select_file(self.rows.files.first().copied());
            return;
        }
        self.pulse.reload(
            fresh,
            same_comparison && self.filter != Filter::Committed,
            &old,
            &self.review,
            &files,
            tx,
        );
        let mapped = |index: usize| files.get(index).copied().flatten();
        let top = self.rows.rows.get(self.visible_rows.start);
        self.collapsed = self.collapsed.iter().copied().filter_map(mapped).collect();
        self.expanded = restore::folds(&old, &self.review, &files, &self.expanded);
        let rows = ReviewRows::new(&self.review, self.filter, &self.collapsed, &self.expanded);
        let target = self
            .active_file
            .and_then(mapped)
            .filter(|&file| rows.header(file).is_some())
            .or_else(|| rows.files.first().copied());
        if let Some(file) = target {
            let row = top
                .filter(|row| mapped(row.file) == Some(file))
                .and_then(|row| {
                    let before = old.files.get(row.file)?;
                    let after = self.review.files.get(file)?;
                    restore::scroll_row(&rows, file, row, before, after)
                });
            if let Some((index, after)) = row.or_else(|| {
                let index = rows.header(file)?;
                rows.rows.get(index).map(|row| (index, row))
            }) {
                if row.is_some() && top.is_some_and(|before| before.key == after.key) {
                    tx.keep_on_screen(&after.key);
                } else {
                    tx.scroll_key_to(&after.key, 0.0);
                }
                self.visible_rows = index..index;
                self.sticky_file = (!matches!(after.kind, RowKind::Header | RowKind::Space)
                    && !self.collapsed.contains(&file))
                .then_some(file);
            }
        } else {
            self.visible_rows = 0..0;
            self.sticky_file = None;
        }
        self.select_file(target);
        self.set_rows(rows);
        tx.instant();
    }

    fn show_error(&mut self, message: String) -> bool {
        match &mut self.status {
            Status::Loading => self.status = Status::Failed(message),
            Status::Failed(error) => {
                if *error == message {
                    return false;
                }
                *error = message;
            }
            Status::Ready { refresh_error } => {
                if refresh_error.as_ref() == Some(&message) {
                    return false;
                }
                *refresh_error = Some(message);
            }
        }
        true
    }

    pub(crate) fn set_filter(&mut self, filter: Filter, tx: &mut Tx) {
        if self.filter == filter {
            tx.live_only();
            return;
        }
        let rows = ReviewRows::new(&self.review, filter, &self.collapsed, &self.expanded);
        let target = self
            .active_file
            .filter(|&file| rows.header(file).is_some())
            .or_else(|| rows.files.first().copied());
        // A file already aligned to the top stays there as the files above it
        // change height. Within a file, hold the first line both filters show.
        let anchor = target
            .filter(|&file| {
                !self
                    .rows
                    .rows
                    .get(self.visible_rows.start)
                    .is_some_and(|row| row.file == file && matches!(row.kind, RowKind::Header))
            })
            .and_then(|file| self.filter_anchor(&rows, file));
        self.pulse.clear();
        self.filter = filter;
        self.sidebar.rebuild(&self.review, filter);
        self.set_rows(rows);
        tx.animated();
        tx.motion(rune::tok::motion::REFLOW);
        self.select_file(target);
        if let Some(anchor) = anchor {
            tx.keep_on_screen(anchor);
        } else if let Some(file) = target {
            self.sticky_file = None;
            self.jump_to_file(file, tx);
        } else {
            self.sticky_file = None;
            self.visible_rows = 0..0;
        }
    }

    fn filter_anchor(&self, rows: &ReviewRows, file: usize) -> Option<String> {
        let next: HashSet<_> = rows
            .rows
            .get(rows.file_rows_range(file)?)?
            .iter()
            .map(|row| row.key.as_str())
            .collect();
        self.rows
            .rows
            .get(self.visible_rows.clone())?
            .iter()
            .find(|row| row.file == file && next.contains(row.key.as_str()))
            .map(|row| row.key.clone())
    }

    pub(crate) fn expand(&mut self, file: usize, range: Range<usize>, tx: &mut Tx) {
        if self.expanded.insert((file, range.start, range.end)) {
            self.rebuild_rows(tx);
        }
    }

    pub(crate) fn toggle_file(&mut self, file: usize, tx: &mut Tx) {
        if !self.collapsed.remove(&file) {
            self.collapsed.insert(file);
        }
        self.rebuild_rows(tx);
        if self.sticky_file == Some(file) {
            self.jump_to_file(file, tx);
        } else {
            tx.keep_on_screen(format!("header/{file}"));
        }
    }

    pub(crate) fn jump_to_file(&mut self, file: usize, tx: &mut Tx) {
        if let Some(index) = self.rows.header(file) {
            self.jump_to_row(index, 0.0, tx);
        }
    }

    /// Scrolling within a file only changes the navigation anchor.
    pub(crate) fn show_rows(&mut self, rows: Range<usize>) -> bool {
        let hint_changed = self.pulse.offscreen(&self.visible_rows) != self.pulse.offscreen(&rows);
        self.visible_rows = rows;
        let top = self.rows.rows.get(self.visible_rows.start);
        let active = self
            .navigation_at_end()
            .and_then(|index| self.rows.rows.get(index))
            .or(top)
            .map(|row| row.file);
        let sticky = top
            .filter(|row| {
                !matches!(row.kind, RowKind::Header | RowKind::Space)
                    && !self.collapsed.contains(&row.file)
            })
            .map(|row| row.file);
        let file_changed = self.active_file != active;
        if file_changed {
            self.select_file(active);
        }
        let sticky_changed = self.sticky_file != sticky;
        self.sticky_file = sticky;
        file_changed || sticky_changed || hint_changed
    }

    /// A jump to a short file near the end can stop with an earlier file at the top.
    fn navigation_at_end(&self) -> Option<usize> {
        self.navigation.filter(|index| {
            self.visible_rows.end == self.rows.rows.len() && self.visible_rows.contains(index)
        })
    }

    fn select_file(&mut self, file: Option<usize>) {
        self.active_file = file;
        self.sidebar_reveal = file;
        if let Some(file) = file {
            self.sidebar.select(file);
        }
    }

    pub(crate) fn sidebar(&self) -> &FileTree {
        &self.sidebar.tree
    }

    pub(crate) fn sidebar_mut(&mut self) -> &mut FileTree {
        &mut self.sidebar.tree
    }

    pub(crate) fn sidebar_select(&mut self, id: &str, tx: &mut Tx) {
        if let Some(file) = self.sidebar.file(id) {
            self.jump_to_file(file, tx);
        }
    }

    pub(crate) fn sidebar_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if matches!(&event.key, Key::Text(name) if matches!(name.as_str(), "j" | "k" | "[" | "]" | "p" | "n" | "r"))
        {
            return self.key(event, tx);
        }
        let (handled, file) = self.sidebar.key(event, tx);
        if let Some(file) = file {
            self.jump_to_file(file, tx);
        }
        handled
    }

    pub(crate) fn toggle_wrap(&mut self, tx: &mut Tx) {
        self.wrap = !self.wrap;
        tx.scroll_to_x("diff-scroll", 0.0);
        tx.instant();
    }

    /// Keeps the active file visible when the sidebar is shorter than its list.
    fn reveal_sidebar(&mut self, tx: &mut Tx) {
        if let Some(file) = self.sidebar_reveal.take() {
            self.sidebar.reveal(file, tx);
        }
    }

    pub(crate) fn key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        #[cfg(feature = "dev")]
        if self.tuning.key(event, self.system_dark.get(), tx) {
            return true;
        }
        if open_shortcut(event)
            && let Some(session) = &self.session
        {
            session.show_start();
            return true;
        }
        if event.mods.cmd || event.mods.ctrl || event.mods.alt {
            return false;
        }
        match &event.key {
            Key::Text(text) => match text.as_str() {
                "j" => self.step_hunk(true, tx),
                "k" => self.step_hunk(false, tx),
                "]" => self.step_file(true, tx),
                "[" => self.step_file(false, tx),
                "p" => self.jump_to_pulse(PulseDirection::Above, tx),
                "n" => self.jump_to_pulse(PulseDirection::Below, tx),
                "r" => self.request_refresh(tx),
                _ => false,
            },
            _ => false,
        }
    }

    fn step_file(&mut self, forward: bool, tx: &mut Tx) -> bool {
        let Some(position) = self
            .active_file
            .and_then(|file| self.rows.files.iter().position(|&index| index == file))
        else {
            return false;
        };
        let next = if forward {
            position.checked_add(1)
        } else {
            position.checked_sub(1)
        };
        let Some(&target) = next.and_then(|next| self.rows.files.get(next)) else {
            return false;
        };
        self.jump_to_file(target, tx);
        true
    }

    fn step_hunk(&mut self, forward: bool, tx: &mut Tx) -> bool {
        let top = self.visible_rows.start;
        let from = self
            .navigation
            .filter(|index| index.abs_diff(top) <= 3)
            .or_else(|| self.navigation_at_end())
            .unwrap_or(top);
        let hunks = &self.rows.hunks;
        let target = if forward {
            hunks.get(hunks.partition_point(|&index| index <= from))
        } else {
            hunks
                .get(..hunks.partition_point(|&index| index < from))
                .and_then(|before| before.last())
        };
        if let Some(&target) = target {
            self.jump_to_row(target, HEADER_HEIGHT, tx);
            true
        } else {
            false
        }
    }

    fn jump_to_row(&mut self, index: usize, at: f32, tx: &mut Tx) {
        let Some(row) = self.rows.rows.get(index) else {
            return;
        };
        self.navigation = Some(index);
        tx.scroll_key_to(&row.key, at);
        if self.active_file != Some(row.file) {
            self.select_file(Some(row.file));
        }
    }

    fn rebuild_rows(&mut self, tx: &mut Tx) {
        let rows = ReviewRows::new(&self.review, self.filter, &self.collapsed, &self.expanded);
        self.set_rows(rows);
        tx.instant();
    }

    fn set_rows(&mut self, rows: ReviewRows) {
        self.rows = rows;
        self.pulse.project(&self.rows, &self.review, self.filter);
        self.rows_version = self.rows_version.wrapping_add(1);
        self.navigation = None;
    }

    pub(crate) fn pulse_offscreen(&self) -> (usize, usize) {
        self.pulse.offscreen(&self.visible_rows)
    }

    pub(crate) fn jump_to_pulse(&mut self, direction: PulseDirection, tx: &mut Tx) -> bool {
        let Some(index) = self.pulse.nearest(&self.visible_rows, direction) else {
            return false;
        };
        self.jump_to_row(index, HEADER_HEIGHT, tx);
        true
    }
}

fn ready() -> Status {
    Status::Ready {
        refresh_error: None,
    }
}
