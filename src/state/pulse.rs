//! One shared opacity track for every mounted row and badge in the pulse.

use std::{ops::Range, time::Duration};

use rune::{Live, Tx};

use crate::model::{
    FileBody, Filter, Review, ReviewRows, RowKind,
    fresh::{self, FreshLines},
};

const PULSE_SECONDS: f32 = 1.2;

pub(crate) struct Pulse {
    lines: FreshLines,
    rows: Vec<usize>,
    opacity: Live<f32>,
    start: f64,
}

impl Default for Pulse {
    fn default() -> Self {
        Self {
            lines: FreshLines::new(),
            rows: Vec::new(),
            opacity: Live::new(0.0),
            start: 0.0,
        }
    }
}

impl Pulse {
    pub(super) fn reload(
        &mut self,
        fresh: FreshLines,
        same_comparison: bool,
        old: &Review,
        new: &Review,
        files: &[Option<usize>],
        tx: &mut Tx,
    ) {
        self.lines = if !same_comparison {
            FreshLines::new()
        } else if fresh.is_empty() {
            fresh::relocate(std::mem::take(&mut self.lines), old, new, files)
        } else {
            self.start = tx.clock();
            self.opacity.set(1.0);
            tx.wake_after(Duration::ZERO);
            fresh
        };
    }

    pub(super) fn poll(&mut self, now: f64, tx: &mut Tx) -> bool {
        if self.lines.is_empty() {
            return false;
        }
        let elapsed = now - self.start;
        if elapsed >= f64::from(PULSE_SECONDS) {
            self.opacity.set(0.0);
            self.lines.clear();
            self.rows.clear();
            tx.instant();
            return true;
        }
        // A short hold and a smooth tint fade. Opacity is the only animated
        // property, so the same pulse works under Reduce Motion.
        #[allow(clippy::cast_possible_truncation, reason = "Live opacity is an f32")]
        let progress = ((elapsed / f64::from(PULSE_SECONDS) - 0.12) / 0.88).clamp(0.0, 1.0) as f32;
        self.opacity
            .set(1.0 - progress * progress * (3.0 - 2.0 * progress));
        tx.wake_after(Duration::ZERO);
        false
    }

    pub(crate) fn opacity(&self) -> &Live<f32> {
        &self.opacity
    }

    pub(crate) fn files(&self) -> impl Iterator<Item = usize> + '_ {
        self.lines.keys().copied()
    }

    pub(crate) fn contains(&self, filter: Filter, file: usize, number: Option<u32>) -> bool {
        filter != Filter::Committed
            && number.is_some_and(|number| {
                self.lines
                    .get(&file)
                    .is_some_and(|lines| lines.contains(&number))
            })
    }

    pub(super) fn project(&mut self, rows: &ReviewRows, review: &Review, filter: Filter) {
        self.rows.clear();
        if filter == Filter::Committed {
            return;
        }
        for (&file, numbers) in &self.lines {
            if let Some(range) = rows.file_rows_range(file)
                && let Some(file) = review.files.get(file)
                && let FileBody::Text(diff) = file.body(filter)
            {
                self.rows.extend(range.filter(|&index| {
                    rows.rows
                        .get(index)
                        .and_then(|row| match row.kind {
                            RowKind::Line(line) => diff.lines.get(line),
                            _ => None,
                        })
                        .and_then(|line| line.worktree)
                        .is_some_and(|number| numbers.contains(&number))
                }));
            }
        }
        self.rows.sort_unstable();
    }

    pub(super) fn offscreen(&self, visible: &Range<usize>) -> (usize, usize) {
        if visible.is_empty() {
            return (0, 0);
        }
        (
            self.rows.partition_point(|&row| row < visible.start),
            self.rows.len() - self.rows.partition_point(|&row| row < visible.end),
        )
    }
}
