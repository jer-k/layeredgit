use std::{ops::Range, rc::Rc};

use rune_kit::syntax::{self, Highlight};

use crate::input::FileVersion;

/// Holds whole-version byte ranges until the alignment picks the source of each line.
pub(super) struct VersionInk {
    lines: Vec<Range<usize>>,
    highlights: Rc<[Highlight]>,
}

impl VersionInk {
    pub(super) fn new(version: &FileVersion) -> Self {
        let text = version.text.as_deref().unwrap_or_default();
        let mut start = 0;
        let lines = text
            .split_terminator('\n')
            .map(|raw| {
                let visible = raw.strip_suffix('\r').unwrap_or(raw);
                let range = start..start + visible.len();
                start += raw.len() + 1;
                range
            })
            .collect();

        Self {
            lines,
            highlights: syntax::highlight(
                syntax::lang_for_path(&version.path.to_string_lossy()),
                text,
            ),
        }
    }

    pub(super) fn line(&self, number: Option<u32>) -> Vec<Highlight> {
        let Some(line) = number
            .and_then(|n| n.checked_sub(1))
            .and_then(|n| self.lines.get(n as usize))
        else {
            return Vec::new();
        };
        let first = self
            .highlights
            .partition_point(|(r, _)| r.end <= line.start);
        self.highlights
            .iter()
            .skip(first)
            .take_while(|(r, _)| r.start < line.end)
            .map(|(r, paint)| {
                let start = r.start.max(line.start) - line.start;
                let end = r.end.min(line.end) - line.start;
                (start..end, *paint)
            })
            .filter(|(r, _)| !r.is_empty())
            .collect()
    }
}

/// Displays tabs at four-column stops while preserving line-local byte ranges.
pub(super) fn expand_tabs(text: &str, mut highlights: Vec<Highlight>) -> (String, Vec<Highlight>) {
    if !text.contains('\t') {
        return (text.into(), highlights);
    }
    let mut shown = String::with_capacity(text.len());
    let mut tabs = Vec::new();
    let mut extra = 0;
    let mut column = 0;
    for (byte, ch) in text.char_indices() {
        if ch == '\t' {
            let width = 4 - column % 4;
            shown.extend(std::iter::repeat_n(' ', width));
            extra += width - 1;
            tabs.push((byte, extra));
            column += width;
        } else {
            shown.push(ch);
            column += 1;
        }
    }
    let shift = |byte| {
        let before = tabs.partition_point(|&(at, _)| at < byte);
        before
            .checked_sub(1)
            .and_then(|i| tabs.get(i))
            .map_or(0, |&(_, n)| n)
    };
    for (range, _) in &mut highlights {
        *range = range.start + shift(range.start)..range.end + shift(range.end);
    }
    (shown, highlights)
}
