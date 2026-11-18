//! One shared opacity track for every mounted row and badge in the pulse.

use std::{ops::Range, time::Duration};

use rune::{Live, Tx};

use crate::model::{
    FileBody, Filter, Review, ReviewRows, RowKind,
    fresh::{self, FreshLines},
};

/// Direction from the current viewport to fresh lines.
#[derive(Clone, Copy)]
pub(crate) enum PulseDirection {
    Above,
    Below,
}

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
        if !same_comparison {
            self.clear();
        } else if fresh.is_empty() {
            self.lines = fresh::relocate(std::mem::take(&mut self.lines), old, new, files);
            if self.lines.is_empty() {
                self.clear();
            }
        } else {
            self.start = tx.clock();
            self.opacity.set(1.0);
            tx.wake_after(Duration::ZERO);
            self.lines = fresh;
        }
    }

    pub(super) fn clear(&mut self) {
        self.lines.clear();
        self.rows.clear();
        self.opacity.set(0.0);
    }

    pub(super) fn poll(&mut self, now: f64, seconds: f32, tx: &mut Tx) -> bool {
        let elapsed = now - self.start;
        if elapsed >= f64::from(seconds) {
            self.clear();
            tx.instant();
            return true;
        }
        // A short hold and a smooth tint fade. Opacity is the only animated
        // property, so the same pulse works under Reduce Motion.
        #[allow(clippy::cast_possible_truncation, reason = "Live opacity is an f32")]
        let progress = ((elapsed / f64::from(seconds) - 0.12) / 0.88).clamp(0.0, 1.0) as f32;
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
        let (above, below) = self.offscreen_rows(visible);
        (above.len(), below.len())
    }

    pub(super) fn nearest(
        &self,
        visible: &Range<usize>,
        direction: PulseDirection,
    ) -> Option<usize> {
        let (above, below) = self.offscreen_rows(visible);
        match direction {
            PulseDirection::Above => above.last(),
            PulseDirection::Below => below.first(),
        }
        .copied()
    }

    fn offscreen_rows(&self, visible: &Range<usize>) -> (&[usize], &[usize]) {
        if visible.is_empty() {
            return (&[], &[]);
        }
        (
            self.rows
                .split_at(self.rows.partition_point(|&row| row < visible.start))
                .0,
            self.rows
                .split_at(self.rows.partition_point(|&row| row < visible.end))
                .1,
        )
    }
}
