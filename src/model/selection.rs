//! File-local selections over the unified diff's display order.

use std::ops::Range;

use super::{FileBody, Filter, Line, Review, ReviewRows, RowKind};

/// A byte offset in a projected code line, including its visual tab expansion.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Point {
    pub(crate) line: usize,
    pub(crate) byte: usize,
}

/// An oriented range in one file, independent of how many rows are mounted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Selection {
    pub(crate) file: usize,
    pub(crate) anchor: Point,
    pub(crate) focus: Point,
}

impl Selection {
    fn ordered(self) -> (Point, Point) {
        (self.anchor.min(self.focus), self.anchor.max(self.focus))
    }

    /// The selected portion of this display row, including empty rows within the range.
    pub(crate) fn bytes(self, file: usize, line: usize, len: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        (file == self.file && start != end && (start.line..=end.line).contains(&line)).then(|| {
            let from = if start.line == line { start.byte } else { 0 };
            let to = if end.line == line { end.byte } else { len };
            from.min(len)..to.min(len)
        })
    }

    /// Copies code rows exactly in the current filter's display order. In All, both sides of
    /// a replacement and the struck-through committed-then-removed lines are ordinary code
    /// rows. Folded lines and metadata never enter the output.
    pub(crate) fn copy(self, review: &Review, rows: &ReviewRows, filter: Filter) -> Option<String> {
        if self.anchor == self.focus {
            return None;
        }
        let mut output = String::new();
        let mut selected = code_rows(review, rows, filter, self.file)
            .filter_map(|(i, line)| Some((line, self.bytes(self.file, i, line.text.len())?)))
            .peekable();
        selected.peek()?;
        while let Some((line, bytes)) = selected.next() {
            line.copy_bytes(bytes, &mut output);
            if selected.peek().is_some() {
                output.push('\n');
            }
        }
        Some(output)
    }

    /// Selects every displayed code row of the file, including rows outside the viewport.
    pub(crate) fn all(
        review: &Review,
        rows: &ReviewRows,
        filter: Filter,
        file: usize,
    ) -> Option<Self> {
        let mut lines = code_rows(review, rows, filter, file);
        let first = lines.next()?;
        let (last, line) = lines.next_back().unwrap_or(first);
        Some(Self {
            file,
            anchor: Point {
                line: first.0,
                byte: 0,
            },
            focus: Point {
                line: last,
                byte: line.text.len(),
            },
        })
    }
}

fn code_rows<'a>(
    review: &'a Review,
    rows: &'a ReviewRows,
    filter: Filter,
    file: usize,
) -> impl DoubleEndedIterator<Item = (usize, &'a Line)> {
    let diff = review
        .files
        .get(file)
        .and_then(|file| match file.body(filter) {
            FileBody::Text(diff) => Some(diff),
            FileBody::Placeholder(_) => None,
        });
    rows.file_rows_range(file)
        .and_then(|range| rows.rows.get(range))
        .into_iter()
        .flatten()
        .filter_map(move |row| match row.kind {
            RowKind::Line(index) => Some((index, diff?.lines.get(index)?)),
            _ => None,
        })
}

impl Line {
    /// Carets fall on a source character boundary, including either end of a tab.
    pub(crate) fn snap_byte(&self, byte: usize) -> usize {
        let byte = self.text.floor_char_boundary(byte);
        let at = self.tabs.partition_point(|tab| tab.end <= byte);
        self.tabs
            .get(at)
            .filter(|tab| tab.contains(&byte))
            .map_or(byte, |tab| {
                if byte - tab.start < tab.end - byte {
                    tab.start
                } else {
                    tab.end
                }
            })
    }

    fn copy_bytes(&self, bytes: Range<usize>, output: &mut String) {
        let mut start = self.snap_byte(bytes.start);
        let end = self.snap_byte(bytes.end);
        let first_tab = self.tabs.partition_point(|tab| tab.end <= start);
        for tab in self
            .tabs
            .iter()
            .skip(first_tab)
            .take_while(|tab| tab.start < end)
        {
            if let Some(text) = self.text.get(start..tab.start) {
                output.push_str(text);
                output.push('\t');
            }
            start = tab.end;
        }
        if let Some(text) = self.text.get(start..end) {
            output.push_str(text);
        }
    }
}
