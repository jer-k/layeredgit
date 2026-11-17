//! Alignment, syntax mapping, and the review data that the view reads.

mod highlighting;
mod rows;

use std::{
    borrow::Cow,
    ops::Range,
    path::{Path, PathBuf},
};

use diff_stack::{DisplayLine, Stack, ThreeWayChange, align};
use rune_kit::syntax::Highlight;

use crate::input::{ChangeKind, FileVersion, InputFile, Placeholder, ReviewInput};

pub(crate) use rows::{ReviewRow, ReviewRows, RowKind};

const BASE: usize = 0;
const HEAD: usize = 1;
const WORKTREE: usize = 2;
const CONTEXT_LINES: usize = 3;

/// Which transition contributes changes to the review.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

    fn transitions(self) -> Option<Range<usize>> {
        match self {
            Self::All => None,
            Self::Committed => Some(HEAD..WORKTREE),
            Self::Uncommitted => Some(WORKTREE..WORKTREE + 1),
        }
    }

    fn includes(self, destination: usize) -> bool {
        self.transitions()
            .is_none_or(|versions| versions.contains(&destination))
    }

    /// Makes changes from an unselected layer read like ordinary context.
    pub(crate) fn appearance(self, change: ThreeWayChange) -> ThreeWayChange {
        use ThreeWayChange::{
            CommittedAdd, CommittedDelete, Context, UncommittedAdd, UncommittedDelete,
        };
        match change {
            CommittedAdd | CommittedDelete if !self.includes(HEAD) => Context,
            UncommittedAdd | UncommittedDelete if !self.includes(WORKTREE) => Context,
            _ => change,
        }
    }
}

/// An owned alignment and syntax cache for a branch review.
#[derive(Default)]
pub(crate) struct Review {
    pub(crate) base_label: String,
    pub(crate) head_label: String,
    pub(crate) files: Vec<FileDiff>,
}

impl Review {
    /// Aligns each file and prepares all syntax and hunk data before the view runs.
    pub(crate) fn new(input: ReviewInput) -> Self {
        Self {
            base_label: input.base_label,
            head_label: input.head_label,
            files: input.files.into_iter().map(FileDiff::new).collect(),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

pub(crate) struct FileDiff {
    paths: [Option<PathBuf>; 3],
    pub(crate) committed: ChangeKind,
    pub(crate) uncommitted: ChangeKind,
    pub(crate) committed_counts: Counts,
    pub(crate) uncommitted_counts: Counts,
    pub(crate) body: FileBody,
}

pub(crate) enum FileBody {
    Text(TextDiff),
    Placeholder(Placeholder),
}

pub(crate) struct TextDiff {
    pub(crate) lines: Vec<Line>,
    hunks: [Vec<Range<usize>>; 3],
    newlines: Vec<Newline>,
}

pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) highlights: Box<[Highlight]>,
    pub(crate) base: Option<u32>,
    pub(crate) head: Option<u32>,
    pub(crate) worktree: Option<u32>,
    pub(crate) change: ThreeWayChange,
}

/// The missing newline's version and the line where it is displayed.
struct Newline {
    line: usize,
    version: usize,
    destination: usize,
}

impl FileDiff {
    fn new(input: InputFile) -> Self {
        let versions = [input.base, input.head, input.worktree];
        let body = if let Some(reason) = input.placeholder {
            FileBody::Placeholder(reason)
        } else {
            TextDiff::new(&versions).map_or(
                FileBody::Placeholder(Placeholder::TooManyLines),
                FileBody::Text,
            )
        };
        let (committed_counts, uncommitted_counts) = match &body {
            FileBody::Text(diff) => counts(&diff.lines),
            FileBody::Placeholder(_) => (Counts::default(), Counts::default()),
        };
        Self {
            paths: versions.map(|version| version.map(|v| v.path)),
            committed: input.committed,
            uncommitted: input.uncommitted,
            committed_counts,
            uncommitted_counts,
            body,
        }
    }

    /// The most recent path, including the former path for a deleted file.
    pub(crate) fn path(&self) -> Cow<'_, str> {
        self.paths()
            .next_back()
            .map_or(Cow::Borrowed(""), Path::to_string_lossy)
    }

    /// Paths in version order, for matching files across refreshes.
    pub(crate) fn paths(&self) -> impl DoubleEndedIterator<Item = &Path> {
        self.paths.iter().filter_map(Option::as_deref)
    }

    /// Finds the old display line in this file, with its index as a fallback.
    pub(crate) fn relocated_line(&self, old: &Self, index: usize) -> Option<usize> {
        let (FileBody::Text(before), FileBody::Text(after)) = (&old.body, &self.body) else {
            return None;
        };
        let anchor = before.lines.get(index)?;
        let rank = |(i, line): (usize, &Line)| {
            (
                line.text != anchor.text,
                line.base != anchor.base,
                line.head != anchor.head,
                line.change != anchor.change,
                i.abs_diff(index),
            )
        };
        if after
            .lines
            .get(index)
            .is_some_and(|line| rank((index, line)) == (false, false, false, false, 0))
        {
            return Some(index);
        }
        after
            .lines
            .iter()
            .enumerate()
            .min_by_key(|&(i, line)| rank((i, line)))
            .map(|(i, _)| i)
    }

    /// The distinct paths in version order, keeping a rename that was reversed.
    pub(crate) fn path_label(&self) -> String {
        let mut paths = Vec::with_capacity(self.paths.len());
        for path in self.paths() {
            if paths.last().copied() != Some(path) {
                paths.push(path);
            }
        }
        paths
            .into_iter()
            .map(Path::to_string_lossy)
            .collect::<Vec<_>>()
            .join(" → ")
    }

    fn has_changes(&self, filter: Filter) -> bool {
        (filter.includes(HEAD) && self.committed != ChangeKind::Unchanged)
            || (filter.includes(WORKTREE) && self.uncommitted != ChangeKind::Unchanged)
    }
}

impl TextDiff {
    fn new(versions: &[Option<FileVersion>; 3]) -> Result<Self, diff_stack::TooManyLines> {
        let texts = versions
            .each_ref()
            .map(|version| version.as_ref().and_then(|v| v.text.as_deref()));
        let stack = align(&texts.map(Option::unwrap_or_default))?;
        let ink = versions
            .each_ref()
            .map(|version| version.as_ref().map(highlighting::VersionInk::new));
        let lines = stack
            .lines()
            .iter()
            .enumerate()
            .filter_map(|(index, shown)| {
                let change = stack.classify_three_way(index)?;
                let source = shown.dies().map_or(WORKTREE, |death| death - 1);
                let highlights = ink
                    .get(source)
                    .and_then(Option::as_ref)
                    .map_or_else(Vec::new, |ink| ink.line(shown.line_number(source)));
                Some(Line::new(shown, change, highlights))
            })
            .collect();
        let mut newlines: Vec<_> = [HEAD, WORKTREE]
            .into_iter()
            .filter_map(|destination| newline(&stack, &texts, destination))
            .collect();
        newlines.sort_by_key(|marker| (marker.line, marker.version));
        Ok(Self {
            lines,
            hunks: Filter::ALL.map(|filter| hunks_with_newlines(&stack, filter, &newlines)),
            newlines,
        })
    }

    fn hunks(&self, filter: Filter) -> &[Range<usize>] {
        match filter {
            Filter::All => &self.hunks[0],
            Filter::Committed => &self.hunks[1],
            Filter::Uncommitted => &self.hunks[2],
        }
    }

    /// Unchanged ranges which can be expanded in this filter.
    pub(crate) fn gaps(&self, filter: Filter) -> impl Iterator<Item = Range<usize>> {
        let hunks = self.hunks(filter);
        let starts = std::iter::once(0).chain(hunks.iter().map(|hunk| hunk.end));
        let ends = hunks
            .iter()
            .map(|hunk| hunk.start)
            .chain(std::iter::once(self.lines.len()));
        starts
            .zip(ends)
            .map(|(start, end)| start..end)
            .filter(move |gap| !hunks.is_empty() && !gap.is_empty())
    }
}

impl Line {
    fn new(line: &DisplayLine<'_>, change: ThreeWayChange, highlights: Vec<Highlight>) -> Self {
        let (text, highlights) = highlighting::expand_tabs(line.text(), highlights);
        Self {
            text,
            highlights: highlights.into_boxed_slice(),
            base: line.line_number(BASE),
            head: line.line_number(HEAD),
            worktree: line.line_number(WORKTREE),
            change,
        }
    }
}

fn newline(stack: &Stack<'_>, texts: &[Option<&str>; 3], destination: usize) -> Option<Newline> {
    let [previous, current] = texts.get(destination.checked_sub(1)?..=destination)? else {
        return None;
    };
    if previous.is_none()
        || current.is_none()
        || stack.trailing_newline_changed(destination) != Some(true)
    {
        return None;
    }
    let version = if stack.trailing_newline(destination) == Some(false) {
        destination
    } else {
        destination - 1
    };
    let line = stack
        .lines()
        .iter()
        .rposition(|line| line.line_number(version).is_some())?;
    Some(Newline {
        line,
        version,
        destination,
    })
}

fn hunks_with_newlines(
    stack: &Stack<'_>,
    filter: Filter,
    markers: &[Newline],
) -> Vec<Range<usize>> {
    let mut hunks = stack.hunks(CONTEXT_LINES, filter.transitions());
    let original_count = hunks.len();
    for marker in markers
        .iter()
        .filter(|marker| filter.includes(marker.destination))
    {
        // A change in the other layer can separate the unterminated line from the
        // newer version's last line, the anchor used by Stack::hunks.
        if !hunks.iter().any(|hunk| hunk.contains(&marker.line)) {
            let start = marker.line.saturating_sub(CONTEXT_LINES);
            let end = (marker.line + CONTEXT_LINES + 1).min(stack.lines().len());
            hunks.push(start..end);
        }
    }
    if hunks.len() == original_count {
        return hunks;
    }

    hunks.sort_unstable_by_key(|hunk| hunk.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(hunks.len());
    for hunk in hunks {
        if let Some(previous) = merged
            .last_mut()
            .filter(|previous| hunk.start <= previous.end)
        {
            previous.end = previous.end.max(hunk.end);
        } else {
            merged.push(hunk);
        }
    }
    merged
}

fn counts(lines: &[Line]) -> (Counts, Counts) {
    use ThreeWayChange::{
        CommittedAdd, CommittedDelete, CommittedThenRemoved, Context, UncommittedAdd,
        UncommittedDelete,
    };

    let mut committed = Counts::default();
    let mut uncommitted = Counts::default();
    for line in lines {
        match line.change {
            Context => {}
            CommittedAdd => committed.added += 1,
            CommittedDelete => committed.removed += 1,
            UncommittedAdd => uncommitted.added += 1,
            UncommittedDelete => uncommitted.removed += 1,
            CommittedThenRemoved => {
                committed.added += 1;
                uncommitted.removed += 1;
            }
        }
    }
    (committed, uncommitted)
}

#[cfg(test)]
mod tests;
