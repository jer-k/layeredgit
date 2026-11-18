//! Alignment, syntax mapping, and the review data that the view reads.

pub(crate) mod fresh;
mod highlighting;
mod projection;
mod rows;
pub(crate) mod selection;

use std::{
    borrow::Cow,
    cmp::Ordering,
    ops::Range,
    path::{Path, PathBuf},
};

use diff_stack::{DisplayLine, Stack, ThreeWayChange, align};
use rune_kit::syntax::Highlight;

use crate::input::{ChangeKind, FileVersion, InputFile, Placeholder, ReviewInput};

pub(crate) use projection::Filter;
use projection::Projection;

pub(crate) use rows::{ReviewRow, ReviewRows, RowKind};

const BASE: usize = 0;
const HEAD: usize = 1;
const WORKTREE: usize = 2;
const CONTEXT_LINES: usize = 3;

/// An owned alignment and syntax cache for a branch review.
#[derive(Default)]
pub(crate) struct Review {
    pub(crate) base_label: String,
    base_revision: [Option<String>; 2],
    pub(crate) head_label: String,
    pub(crate) files: Vec<FileDiff>,
    order: [Vec<usize>; 3],
}

impl Review {
    /// Aligns each file and prepares all syntax and hunk data before the view runs.
    pub(crate) fn new(input: ReviewInput) -> Self {
        let files: Vec<_> = input.files.into_iter().map(FileDiff::new).collect();
        let order = Filter::ALL.map(|filter| {
            let mut order: Vec<_> = files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.has_changes(filter))
                .collect();
            order.sort_by(|(_, left), (_, right)| {
                tree_order(left.display_path(filter), right.display_path(filter))
            });
            order.into_iter().map(|(index, _)| index).collect()
        });
        Self {
            base_label: input.base_label,
            base_revision: input.base_revision,
            head_label: input.head_label,
            files,
            order,
        }
    }

    pub(crate) fn file_order(&self, filter: Filter) -> &[usize] {
        match filter {
            Filter::All => &self.order[0],
            Filter::Committed => &self.order[1],
            Filter::Uncommitted => &self.order[2],
        }
    }
}

fn tree_order(left: &Path, right: &Path) -> Ordering {
    let mut left = left.components().peekable();
    let mut right = right.components().peekable();
    while let (Some(a), Some(b)) = (left.next(), right.next()) {
        let a = a.as_os_str();
        let b = b.as_os_str();
        let order = right
            .peek()
            .is_some()
            .cmp(&left.peek().is_some())
            .then_with(|| {
                rune_kit::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy())
                    .then_with(|| a.cmp(b))
            });
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

pub(crate) struct FileDiff {
    paths: [Option<PathBuf>; 3],
    /// HEAD's text: the previous working text of a file that was clean before a reload.
    head_text: Option<String>,
    worktree_text: Option<String>,
    pub(crate) committed: ChangeKind,
    pub(crate) uncommitted: ChangeKind,
    pub(crate) committed_counts: Counts,
    pub(crate) uncommitted_counts: Counts,
    text: Result<TextDiff, Placeholder>,
    placeholders: [Option<Placeholder>; 2],
}

pub(crate) enum FileBody<'a> {
    Text(&'a TextDiff),
    Placeholder(Placeholder),
}

pub(crate) struct TextDiff {
    pub(crate) lines: Vec<Line>,
    projections: [Projection; 3],
    newlines: Vec<Newline>,
}

pub(crate) struct Line {
    pub(crate) text: String,
    /// Display byte ranges taken up by source tabs.
    pub(crate) tabs: Box<[Range<usize>]>,
    columns: usize,
    highlights: Box<[Highlight]>,
    head_highlights: Option<Box<[Highlight]>>,
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
        let mut versions = [input.base, input.head, input.worktree];
        let text = match input.placeholders {
            [Some(before), Some(after)] => Err(before.max(after)),
            _ => TextDiff::new(&versions).map_err(|_| Placeholder::TooManyLines),
        };
        let (committed_counts, uncommitted_counts) = text.as_ref().map_or_else(
            |_| (Counts::default(), Counts::default()),
            |diff| counts(&diff.lines),
        );
        Self {
            head_text: versions[HEAD]
                .as_mut()
                .and_then(|version| version.text.take()),
            worktree_text: versions[WORKTREE]
                .as_mut()
                .and_then(|version| version.text.take()),
            paths: versions.map(|version| version.map(|v| v.path)),
            committed: input.committed,
            uncommitted: input.uncommitted,
            committed_counts,
            uncommitted_counts,
            text,
            placeholders: input.placeholders,
        }
    }

    /// The latest path in the selected comparison, including the path of a deletion.
    pub(crate) fn display_path(&self, filter: Filter) -> &Path {
        let (start, end) = filter.columns();
        self.paths
            .get(start..=end)
            .into_iter()
            .flatten()
            .filter_map(Option::as_deref)
            .next_back()
            .unwrap_or_else(|| Path::new(""))
    }

    #[cfg(test)]
    pub(crate) fn path(&self) -> Cow<'_, str> {
        self.display_path(Filter::All).to_string_lossy()
    }

    /// Paths in version order, for matching files across refreshes.
    pub(crate) fn paths(&self) -> impl DoubleEndedIterator<Item = &Path> {
        self.paths.iter().filter_map(Option::as_deref)
    }

    /// Finds the old display line in this file, with its index as a fallback.
    pub(crate) fn relocated_line(&self, old: &Self, index: usize) -> Option<usize> {
        let before = old.text.as_ref().ok()?;
        let after = self.text.as_ref().ok()?;
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
    pub(crate) fn path_label(&self, filter: Filter) -> String {
        let mut paths = Vec::with_capacity(self.paths.len());
        let (start, end) = filter.columns();
        for path in self
            .paths
            .get(start..=end)
            .into_iter()
            .flatten()
            .filter_map(Option::as_deref)
        {
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

    pub(crate) fn body(&self, filter: Filter) -> FileBody<'_> {
        let [committed, uncommitted] = self.placeholders;
        let placeholder = match filter {
            Filter::All => committed.max(uncommitted),
            Filter::Committed => committed,
            Filter::Uncommitted => uncommitted,
        };
        match (placeholder, &self.text) {
            (Some(reason), _) => FileBody::Placeholder(reason),
            (None, Err(reason)) => FileBody::Placeholder(*reason),
            (None, Ok(diff)) => FileBody::Text(diff),
        }
    }

    /// The active change badge and whether working-tree provenance is visible.
    pub(crate) fn badge(&self, filter: Filter) -> (ChangeKind, bool) {
        let working = filter.includes(WORKTREE) && self.uncommitted != ChangeKind::Unchanged;
        (
            if working {
                self.uncommitted
            } else {
                self.committed
            },
            working,
        )
    }

    /// The kind, counts, and optional label for a layer that changed.
    pub(crate) fn summary(&self, filter: Filter) -> Option<(&Counts, Option<Cow<'static, str>>)> {
        let (kind, counts) = match filter {
            Filter::Committed => (self.committed, &self.committed_counts),
            Filter::Uncommitted => (self.uncommitted, &self.uncommitted_counts),
            Filter::All => return None,
        };
        if kind == ChangeKind::Unchanged {
            return None;
        }
        let label = match self.body(filter) {
            FileBody::Placeholder(Placeholder::Binary) => Some("Binary".into()),
            FileBody::Placeholder(reason) => Some(reason.message()),
            FileBody::Text(diff) if *counts == Counts::default() => Some(
                if diff
                    .newlines
                    .iter()
                    .any(|marker| filter.includes(marker.destination))
                {
                    "Newline at end of file"
                } else {
                    match kind {
                        ChangeKind::ModeChanged => "Mode change",
                        ChangeKind::Renamed => "Renamed",
                        ChangeKind::Added => "Added",
                        ChangeKind::Deleted => "Deleted",
                        _ => "Modified",
                    }
                }
                .into(),
            ),
            FileBody::Text(_) => None,
        };
        Some((counts, label))
    }

    pub(super) fn has_changes(&self, filter: Filter) -> bool {
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
        let lines: Vec<_> = stack
            .lines()
            .iter()
            .enumerate()
            .filter_map(|(index, shown)| {
                let change = stack.classify_three_way(index)?;
                let source = shown.dies().map_or(WORKTREE, |death| death - 1);
                let syntax = |version| {
                    let number = shown.line_number(version)?;
                    ink.get(version)?.as_ref().map(|ink| ink.line(Some(number)))
                };
                let highlights = syntax(source).unwrap_or_default();
                let head_highlights = (source == WORKTREE)
                    .then(|| syntax(HEAD))
                    .flatten()
                    .filter(|head| head != &highlights);
                Some(Line::new(shown, change, highlights, head_highlights))
            })
            .collect();
        let mut newlines: Vec<_> = [HEAD, WORKTREE]
            .into_iter()
            .filter_map(|destination| newline(&stack, &texts, destination))
            .collect();
        newlines.sort_by_key(|marker| (marker.line, marker.version));
        let projections = Filter::ALL.map(|filter| {
            if filter == Filter::All {
                Projection::all(hunks_with_newlines(&stack, &newlines))
            } else {
                Projection::pair(&lines, &newlines, filter)
            }
        });
        Ok(Self {
            lines,
            projections,
            newlines,
        })
    }

    fn projection(&self, filter: Filter) -> &Projection {
        match filter {
            Filter::All => &self.projections[0],
            Filter::Committed => &self.projections[1],
            Filter::Uncommitted => &self.projections[2],
        }
    }

    fn hunks(&self, filter: Filter) -> &[Range<usize>] {
        &self.projection(filter).hunks
    }

    pub(crate) fn visible_count(&self, filter: Filter, range: Range<usize>) -> usize {
        self.projection(filter).count(range)
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
            .filter(move |gap| !hunks.is_empty() && self.visible_count(filter, gap.clone()) > 0)
    }
}

impl Line {
    pub(crate) fn highlights(&self, filter: Filter) -> &[Highlight] {
        if filter == Filter::Committed
            && let Some(head) = &self.head_highlights
        {
            return head;
        }
        &self.highlights
    }

    pub(crate) fn number(&self, version: usize) -> Option<u32> {
        match version {
            BASE => self.base,
            HEAD => self.head,
            WORKTREE => self.worktree,
            _ => None,
        }
    }

    fn new(
        line: &DisplayLine<'_>,
        change: ThreeWayChange,
        highlights: Vec<Highlight>,
        head_highlights: Option<Vec<Highlight>>,
    ) -> Self {
        let (text, highlights, tabs) = highlighting::expand_tabs(line.text(), highlights);
        let head_highlights = head_highlights.map(|highlights| {
            highlighting::expand_tabs(line.text(), highlights)
                .1
                .into_boxed_slice()
        });
        // Allow two cells for wide glyphs when estimating the unwrapped extent.
        let columns = text
            .chars()
            .map(|ch| if ch.len_utf8() >= 3 { 2 } else { 1 })
            .sum();
        Self {
            text,
            tabs,
            columns,
            highlights: highlights.into_boxed_slice(),
            head_highlights,
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

fn hunks_with_newlines(stack: &Stack<'_>, markers: &[Newline]) -> Vec<Range<usize>> {
    let mut hunks = stack.hunks(CONTEXT_LINES, None);
    let original_count = hunks.len();
    for marker in markers {
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
