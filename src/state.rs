//! Review interactions and the current virtual list projection.

use std::{collections::HashSet, ops::Range};

use rune::{Key, KeyEvent, Tx};

use crate::{
    input::ReviewInput,
    model::{Filter, Review, ReviewRows, RowKind},
};

pub(crate) const HEADER_HEIGHT: f32 = 44.0;

pub(crate) struct State {
    pub(crate) review: Review,
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
    /// Prepares the review before the first view is built.
    pub(crate) fn new(input: ReviewInput) -> Self {
        let review = Review::new(input);
        let filter = Filter::All;
        let collapsed = HashSet::new();
        let expanded = HashSet::new();
        let rows = ReviewRows::new(&review, filter, &collapsed, &expanded);
        let active_file = (!review.files.is_empty()).then_some(0);
        Self {
            review,
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
    pub(crate) fn reveal_sidebar(&mut self, _: f64, tx: &mut Tx) -> bool {
        if let Some(file) = self.sidebar_reveal.take() {
            tx.scroll_into_view_in("sidebar-scroll", format!("file/{file}"));
        }
        false
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
        self.rows = ReviewRows::new(&self.review, self.filter, &self.collapsed, &self.expanded);
        self.rows_version = self.rows_version.wrapping_add(1);
        self.navigation = None;
        tx.instant();
    }
}
