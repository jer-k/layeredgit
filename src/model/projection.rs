use std::ops::Range;

use diff_stack::ThreeWayChange;

use super::{BASE, CONTEXT_LINES, HEAD, Line, Newline, WORKTREE};

/// The versions and changes to show together.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Filter {
    #[default]
    All,
    Committed,
    Uncommitted,
}

impl Filter {
    pub(crate) const ALL: [Self; 3] = [Self::All, Self::Committed, Self::Uncommitted];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Committed => "Committed",
            Self::Uncommitted => "Uncommitted",
        }
    }

    pub(super) fn includes(self, destination: usize) -> bool {
        self == Self::All || self.columns().1 == destination
    }

    pub(crate) fn columns(self) -> (usize, usize) {
        match self {
            Self::All => (BASE, WORKTREE),
            Self::Committed => (BASE, HEAD),
            Self::Uncommitted => (HEAD, WORKTREE),
        }
    }

    pub(crate) fn column_labels(self) -> (&'static str, &'static str) {
        const LABELS: [&str; 3] = ["Base", "HEAD", "Worktree"];
        let (old, new) = self.columns();
        (LABELS[old], LABELS[new])
    }

    /// A missing line number means the line's lifetime excludes that endpoint.
    pub(crate) fn change(self, line: &Line) -> Option<ThreeWayChange> {
        use ThreeWayChange::{
            CommittedAdd, CommittedDelete, Context, UncommittedAdd, UncommittedDelete,
        };
        if self == Self::All {
            return Some(line.change);
        }
        let (old, new) = self.columns();
        match (line.number(old).is_some(), line.number(new).is_some()) {
            (false, false) => None,
            (true, true) => Some(Context),
            (false, true) if self == Self::Committed => Some(CommittedAdd),
            (true, false) if self == Self::Committed => Some(CommittedDelete),
            (false, true) => Some(UncommittedAdd),
            (true, false) => Some(UncommittedDelete),
        }
    }
}

/// Hunk boundaries use alignment indices so line keys survive filter switches and refreshes.
pub(super) struct Projection {
    visible: Option<Vec<usize>>,
    pub(super) hunks: Vec<Range<usize>>,
}

impl Projection {
    pub(super) fn all(hunks: Vec<Range<usize>>) -> Self {
        Self {
            visible: None,
            hunks,
        }
    }

    pub(super) fn pair(lines: &[Line], newlines: &[Newline], filter: Filter) -> Self {
        let mut visible = Vec::new();
        let mut hunks: Vec<Range<usize>> = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let Some(change) = filter.change(line) else {
                continue;
            };
            let position = visible.len();
            visible.push(index);
            if change != ThreeWayChange::Context
                || newlines
                    .iter()
                    .any(|marker| marker.line == index && filter.includes(marker.destination))
            {
                let start = position.saturating_sub(CONTEXT_LINES);
                let end = position + CONTEXT_LINES + 1;
                if let Some(last) = hunks.last_mut().filter(|last| start <= last.end) {
                    last.end = end;
                } else {
                    hunks.push(start..end);
                }
            }
        }
        let hunks = hunks
            .into_iter()
            .filter_map(|range| {
                let start = *visible.get(range.start)?;
                let end = visible
                    .get(range.end - 1)
                    .map_or(lines.len(), |&last| last + 1);
                Some(start..end)
            })
            .collect();
        Self {
            visible: Some(visible),
            hunks,
        }
    }

    pub(super) fn count(&self, range: Range<usize>) -> usize {
        self.visible.as_ref().map_or(range.len(), |visible| {
            visible.partition_point(|&index| index < range.end)
                - visible.partition_point(|&index| index < range.start)
        })
    }
}
