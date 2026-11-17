use std::{borrow::Cow, collections::HashSet, ops::Range};

use super::{BASE, FileBody, FileDiff, Filter, HEAD, Line, Review, TextDiff, WORKTREE};

/// A keyed projection of the diff, rebuilt only when its structure changes.
#[derive(Default)]
pub(crate) struct ReviewRows {
    pub(crate) rows: Vec<ReviewRow>,
    pub(crate) headers: Vec<usize>,
    pub(crate) hunks: Vec<usize>,
}

pub(crate) struct ReviewRow {
    pub(crate) file: usize,
    pub(crate) key: String,
    pub(crate) kind: RowKind,
}

pub(crate) enum RowKind {
    Header,
    Hunk { start: usize, label: String },
    Line(usize),
    Fold(Range<usize>),
    Newline(&'static str),
    Placeholder(Cow<'static, str>),
    Space,
}

impl ReviewRows {
    pub(crate) fn new(
        review: &Review,
        filter: Filter,
        collapsed: &HashSet<usize>,
        expanded: &HashSet<(usize, usize, usize)>,
    ) -> Self {
        let mut rows = Self::default();
        for (index, file) in review.files.iter().enumerate() {
            rows.headers.push(rows.rows.len());
            rows.push(index, format!("header/{index}"), RowKind::Header);
            if !collapsed.contains(&index) {
                rows.file_rows(index, file, filter, expanded);
            }
            rows.push(index, format!("space/{index}"), RowKind::Space);
        }
        rows
    }

    fn file_rows(
        &mut self,
        index: usize,
        file: &FileDiff,
        filter: Filter,
        expanded: &HashSet<(usize, usize, usize)>,
    ) {
        let message = match &file.body {
            FileBody::Text(diff) if !diff.hunks(filter).is_empty() => {
                self.text_rows(index, diff, filter, expanded);
                return;
            }
            _ if !file.has_changes(filter) => Cow::Borrowed(match filter {
                Filter::All => "No changes",
                Filter::Committed => "No committed changes",
                Filter::Uncommitted => "No uncommitted changes",
            }),
            FileBody::Placeholder(reason) => reason.message(),
            FileBody::Text(_) => Cow::Borrowed("No textual changes"),
        };
        self.push(
            index,
            format!("summary/{index}"),
            RowKind::Placeholder(message),
        );
    }

    fn text_rows(
        &mut self,
        file: usize,
        diff: &TextDiff,
        filter: Filter,
        expanded: &HashSet<(usize, usize, usize)>,
    ) {
        let mut end = 0;
        for hunk in diff.hunks(filter) {
            self.gap(file, diff, filter, expanded, end..hunk.start);
            self.hunks.push(self.rows.len());
            self.push(
                file,
                format!("hunk/{file}/{}", hunk.start),
                RowKind::Hunk {
                    start: hunk.start,
                    label: hunk_label(&diff.lines, hunk.clone()),
                },
            );
            self.lines(file, diff, filter, hunk.clone());
            end = hunk.end;
        }
        self.gap(file, diff, filter, expanded, end..diff.lines.len());
    }

    fn gap(
        &mut self,
        file: usize,
        diff: &TextDiff,
        filter: Filter,
        expanded: &HashSet<(usize, usize, usize)>,
        range: Range<usize>,
    ) {
        if range.is_empty() {
            return;
        }
        if expanded.contains(&(file, range.start, range.end)) {
            self.lines(file, diff, filter, range);
        } else {
            self.push(
                file,
                format!("fold/{file}/{}", range.start),
                RowKind::Fold(range),
            );
        }
    }

    fn lines(&mut self, file: usize, diff: &TextDiff, filter: Filter, range: Range<usize>) {
        let mut newlines = diff
            .newlines
            .iter()
            .filter(|marker| filter.includes(marker.destination))
            .peekable();
        for line in range {
            self.push(file, format!("line/{file}/{line}"), RowKind::Line(line));
            while newlines.next_if(|marker| marker.line < line).is_some() {}
            let mut shown = None;
            while let Some(marker) = newlines.next_if(|marker| marker.line == line) {
                if shown == Some(marker.version) {
                    continue;
                }
                shown = Some(marker.version);
                let version = match marker.version {
                    BASE => "Base",
                    HEAD => "HEAD",
                    WORKTREE => "Working tree",
                    _ => continue,
                };
                self.push(
                    file,
                    format!("eof/{file}/{}", marker.version),
                    RowKind::Newline(version),
                );
            }
        }
    }

    fn push(&mut self, file: usize, key: String, kind: RowKind) {
        self.rows.push(ReviewRow { file, key, kind });
    }
}

fn hunk_label(lines: &[Line], range: Range<usize>) -> String {
    let bounds = |number: fn(&Line) -> Option<u32>| {
        let mut present = lines
            .get(range.clone())
            .into_iter()
            .flatten()
            .filter_map(number);
        if let Some(first) = present.next() {
            let last = present.next_back().unwrap_or(first);
            (first, last - first + 1)
        } else {
            let before = lines
                .get(..range.start)
                .into_iter()
                .flatten()
                .rev()
                .find_map(number);
            (before.unwrap_or(0), 0)
        }
    };
    let (base, old_count) = bounds(|line| line.base);
    let (worktree, new_count) = bounds(|line| line.worktree);
    format!("@@ -{base},{old_count} +{worktree},{new_count} @@")
}
