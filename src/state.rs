//! Review interactions and the current virtual list projection.

mod restore;
#[cfg(test)]
mod tests;

use std::{collections::HashSet, ops::Range};

use rune::{Key, KeyEvent, Tx, Waker};

use crate::{
    model::{Filter, Review, ReviewRows, RowKind},
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
    pub(crate) review: Review,
    pub(crate) status: Status,
    previous_review: Option<Review>,
    refresh: Option<Refresh>,
    pub(crate) filter: Filter,
    pub(crate) rows: ReviewRows,
    pub(crate) rows_version: u64,
    pub(crate) active_file: Option<usize>,
    pub(crate) sticky_file: Option<usize>,
    pub(crate) collapsed: HashSet<usize>,
    expanded: HashSet<(usize, usize, usize)>,
    top_row: usize,
    navigation: Option<usize>,
    sidebar_reveal: Option<usize>,
}

impl State {
    pub(crate) fn open(repository: Option<Repository>, headless: bool) -> Self {
        match repository {
            None => Self::new(Review::new(crate::demo::review()), ready(), None),
            Some(repository) if headless => match repository.load_input() {
                Ok(input) => Self::new(Review::new(input), ready(), None),
                Err(error) => Self::new(Review::default(), Status::Failed(error.to_string()), None),
            },
            Some(repository) => Self::new(
                Review::default(),
                Status::Loading,
                Some(Refresh::new(repository)),
            ),
        }
    }

    fn new(review: Review, status: Status, refresh: Option<Refresh>) -> Self {
        let filter = Filter::All;
        let collapsed = HashSet::new();
        let expanded = HashSet::new();
        let rows = ReviewRows::new(&review, filter, &collapsed, &expanded);
        let active_file = (!review.files.is_empty()).then_some(0);
        Self {
            review,
            status,
            previous_review: None,
            refresh,
            filter,
            rows,
            rows_version: 0,
            active_file,
            sticky_file: None,
            collapsed,
            expanded,
            top_row: 0,
            navigation: None,
            sidebar_reveal: None,
        }
    }

    pub(crate) fn can_refresh(&self) -> bool {
        self.refresh.is_some()
    }

    pub(crate) fn setup(&mut self, waker: Waker) {
        if let Some(refresh) = &self.refresh {
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

    pub(crate) fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        self.reveal_sidebar(tx);
        match self
            .refresh
            .as_mut()
            .and_then(|refresh| refresh.poll(now, tx))
        {
            Some(Ok(review)) => {
                self.replace_review(review, tx);
                true
            }
            Some(Err(error)) => self.show_error(error.to_string()),
            None => false,
        }
    }

    fn replace_review(&mut self, review: Review, tx: &mut Tx) {
        let old = std::mem::replace(&mut self.review, review);
        let loaded = matches!(self.status, Status::Ready { .. });
        self.status = ready();
        if !loaded {
            self.active_file = (!self.review.files.is_empty()).then_some(0);
            self.rebuild_rows(tx);
            return;
        }
        // Retain both alignments for matching lines across refreshes.
        let old = self.previous_review.insert(old);
        let files = restore::match_files(old, &self.review);
        let mapped = |index: usize| files.get(index).copied().flatten();
        let top = self.rows.rows.get(self.top_row);
        let target = self.active_file.and_then(mapped).or_else(|| {
            self.review
                .files
                .len()
                .checked_sub(1)
                .map(|last| self.active_file.unwrap_or(0).min(last))
        });
        self.collapsed = self.collapsed.iter().copied().filter_map(mapped).collect();
        self.expanded = restore::folds(old, &self.review, &files, &self.expanded);
        let rows = ReviewRows::new(&self.review, self.filter, &self.collapsed, &self.expanded);
        if let Some(file) = target {
            let row = top
                .filter(|row| mapped(row.file) == Some(file))
                .and_then(|row| {
                    let before = old.files.get(row.file)?;
                    let after = self.review.files.get(file)?;
                    restore::scroll_row(&rows, file, row, before, after)
                });
            if let Some((index, after)) = row.or_else(|| {
                let &index = rows.headers.get(file)?;
                rows.rows.get(index).map(|row| (index, row))
            }) {
                let key = format!("diff-scroll/diff-rows/{}", after.key);
                if row.is_some() && top.is_some_and(|before| before.key == after.key) {
                    tx.keep_on_screen(key);
                } else {
                    tx.scroll_key_to(key, 0.0);
                }
                self.top_row = index;
                self.sticky_file = (!matches!(after.kind, RowKind::Header)).then_some(file);
            }
        } else {
            self.top_row = 0;
            self.sticky_file = None;
        }
        self.active_file = target;
        self.sidebar_reveal = target;
        self.set_rows(rows, tx);
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
        self.filter = filter;
        self.rebuild_rows(tx);
        if let Some(file) = self.active_file {
            self.jump_to_file(file, tx);
        }
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
        if let Some(&index) = self.rows.headers.get(file) {
            self.jump_to_row(index, 0.0, tx);
        }
    }

    /// Scrolling inside a file only updates bookkeeping. Crossing a header updates the UI.
    pub(crate) fn show_rows(&mut self, rows: Range<usize>) -> bool {
        self.top_row = rows.start;
        let active = self.rows.rows.get(rows.start).map(|row| row.file);
        let sticky = self
            .rows
            .rows
            .get(rows.start)
            .filter(|row| !matches!(row.kind, RowKind::Header))
            .map(|row| row.file);
        let file_changed = self.active_file != active;
        let changed = file_changed || self.sticky_file != sticky;
        if file_changed {
            self.sidebar_reveal = active;
        }
        self.active_file = active;
        self.sticky_file = sticky;
        changed
    }

    /// Keeps the active file visible when the sidebar is shorter than its list.
    fn reveal_sidebar(&mut self, tx: &mut Tx) {
        if let Some(file) = self.sidebar_reveal.take() {
            tx.scroll_into_view_in("sidebar-scroll", format!("file/{file}"));
        }
    }

    pub(crate) fn key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if event.mods.cmd || event.mods.ctrl || event.mods.alt {
            return false;
        }
        match &event.key {
            Key::Text(text) => match text.as_str() {
                "j" => self.step_hunk(true, tx),
                "k" => self.step_hunk(false, tx),
                "]" => self.step_file(true, tx),
                "[" => self.step_file(false, tx),
                "r" => self.request_refresh(tx),
                _ => false,
            },
            _ => false,
        }
    }

    fn step_file(&mut self, forward: bool, tx: &mut Tx) -> bool {
        let Some(target) = self
            .active_file
            .and_then(|file| {
                if forward {
                    file.checked_add(1)
                } else {
                    file.checked_sub(1)
                }
            })
            .filter(|&file| file < self.review.files.len())
        else {
            return false;
        };
        self.jump_to_file(target, tx);
        true
    }

    fn step_hunk(&mut self, forward: bool, tx: &mut Tx) -> bool {
        let from = self
            .navigation
            .filter(|index| index.abs_diff(self.top_row) <= 3)
            .unwrap_or(self.top_row);
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
        tx.scroll_key_to(format!("diff-scroll/diff-rows/{}", row.key), at);
    }

    fn rebuild_rows(&mut self, tx: &mut Tx) {
        let rows = ReviewRows::new(&self.review, self.filter, &self.collapsed, &self.expanded);
        self.set_rows(rows, tx);
    }

    fn set_rows(&mut self, rows: ReviewRows, tx: &mut Tx) {
        self.rows = rows;
        self.rows_version = self.rows_version.wrapping_add(1);
        self.navigation = None;
        tx.instant();
    }
}

fn ready() -> Status {
    Status::Ready {
        refresh_error: None,
    }
}
