use std::{borrow::Cow, collections::HashSet, ops::Range};

use super::{BASE, FileBody, FileDiff, Filter, HEAD, Line, Review, TextDiff, WORKTREE};

/// A keyed projection of the diff, rebuilt only when its structure changes.
#[derive(Default)]
pub(crate) struct ReviewRows {
    pub(crate) rows: Vec<ReviewRow>,
    pub(crate) files: Vec<usize>,
    headers: Vec<usize>,
    header_by_file: Vec<Option<usize>>,
    pub(crate) hunks: Vec<usize>,
    pub(crate) widest_line: usize,
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
    Fold { range: Range<usize>, count: usize },
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
        let mut rows = Self {
            header_by_file: vec![None; review.files.len()],
            ..Self::default()
        };
        for &index in review.file_order(filter) {
            let Some(file) = review.files.get(index) else {
                continue;
            };
            rows.files.push(index);
            rows.headers.push(rows.rows.len());
            if let Some(header) = rows.header_by_file.get_mut(index) {
                *header = Some(rows.rows.len());
            }
            rows.push(index, format!("header/{index}"), RowKind::Header);
            if !collapsed.contains(&index) {
                rows.file_rows(index, file, filter, expanded);
            }
            rows.push(index, format!("space/{index}"), RowKind::Space);
        }
        rows
    }

    pub(crate) fn header(&self, file: usize) -> Option<usize> {
        self.header_by_file.get(file).copied().flatten()
    }

    pub(crate) fn file_rows_range(&self, file: usize) -> Option<Range<usize>> {
        let start = self.header(file)?;
        let end = self
            .headers
            .get(self.headers.partition_point(|&row| row <= start))
            .copied()
            .unwrap_or(self.rows.len());
        Some(start..end)
    }

    fn file_rows(
        &mut self,
        index: usize,
        file: &FileDiff,
        filter: Filter,
        expanded: &HashSet<(usize, usize, usize)>,
    ) {
        let message = match file.body(filter) {
            FileBody::Text(diff) if !diff.hunks(filter).is_empty() => {
                self.text_rows(index, diff, filter, expanded);
                return;
            }
            FileBody::Placeholder(reason) => reason.message(),
            FileBody::Text(_) => [Filter::Committed, Filter::Uncommitted]
                .into_iter()
                .filter(|&layer| filter == Filter::All || filter == layer)
                .find_map(|layer| file.summary(layer).and_then(|(_, label)| label))
                .unwrap_or(Cow::Borrowed("No textual changes")),
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
                    label: hunk_label(&diff.lines, hunk.clone(), filter),
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
        let count = diff.visible_count(filter, range.clone());
        if count == 0 {
            return;
        }
        if expanded.contains(&(file, range.start, range.end)) {
            self.lines(file, diff, filter, range);
        } else {
            self.push(
                file,
                format!("fold/{file}/{}", range.start),
                RowKind::Fold { range, count },
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
            let Some(content) = diff
                .lines
                .get(line)
                .filter(|line| filter.change(line).is_some())
            else {
                continue;
            };
            self.widest_line = self.widest_line.max(content.columns);
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

fn hunk_label(lines: &[Line], range: Range<usize>, filter: Filter) -> String {
    let bounds = |version| {
        let number = |line: &Line| line.number(version);
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
    let (old, new) = filter.columns();
    let (old, old_count) = bounds(old);
    let (new, new_count) = bounds(new);
    format!("@@ -{old},{old_count} +{new},{new_count} @@")
}
