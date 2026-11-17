//! Aligns a chain of text versions (for example merge-base -> HEAD -> working tree)
//! and labels every displayed line with the span of versions it exists in.

use std::fmt;
use std::mem;
use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};

/// An alignment of text versions in display order.
#[derive(Debug, Default)]
pub struct Stack<'a> {
    lines: Vec<DisplayLine<'a>>,
    trailing_newlines: Vec<bool>,
}

impl<'a> Stack<'a> {
    /// Returns all display lines, including lines that were later deleted.
    pub fn lines(&self) -> &[DisplayLine<'a>] {
        &self.lines
    }

    /// Returns the number of input versions.
    pub fn version_count(&self) -> usize {
        self.trailing_newlines.len()
    }

    /// Returns whether the given version ends in `\n`, if the version exists.
    pub fn trailing_newline(&self, version: usize) -> Option<bool> {
        self.trailing_newlines.get(version).copied()
    }

    /// Returns whether the trailing newline flag differs from the previous version.
    ///
    /// Returns `None` for version zero and for an out-of-bounds version index.
    pub fn trailing_newline_changed(&self, version: usize) -> Option<bool> {
        let previous = self.trailing_newline(version.checked_sub(1)?)?;
        Some(previous != self.trailing_newline(version)?)
    }

    /// Classifies a display line for a three-version base, HEAD, and worktree stack.
    ///
    /// Returns `None` for any other stack length or an out-of-bounds display index.
    pub fn classify_three_way(&self, display_index: usize) -> Option<ThreeWayChange> {
        if self.version_count() != 3 {
            return None;
        }
        let line = self.lines.get(display_index)?;
        match (line.born, line.dies) {
            (0, None) => Some(ThreeWayChange::Context),
            (1, None) => Some(ThreeWayChange::CommittedAdd),
            (0, Some(1)) => Some(ThreeWayChange::CommittedDelete),
            (2, None) => Some(ThreeWayChange::UncommittedAdd),
            (0, Some(2)) => Some(ThreeWayChange::UncommittedDelete),
            (1, Some(2)) => Some(ThreeWayChange::CommittedThenRemoved),
            _ => None,
        }
    }

    /// Returns the display-line index ranges of the visible hunks.
    ///
    /// `context` counts display lines on either side of each changed line. Ranges
    /// that overlap or touch are merged. With `None`, all births after version
    /// zero and all deaths count as changes. A transition filter counts only
    /// births and deaths whose destination version is in the supplied range.
    /// For example, `Some(2..3)` selects HEAD-to-worktree changes in a three-version
    /// stack. Other display lines can still appear as context.
    ///
    /// A selected version whose trailing-newline flag differs from the previous
    /// version also marks one line as changed: its last live line, or the previous
    /// version's last live line if it has none. That is where the UI shows
    /// "No newline at end of file".
    pub fn hunks(&self, context: usize, transitions: Option<Range<usize>>) -> Vec<Range<usize>> {
        let transitions = transitions.unwrap_or(1..self.version_count());
        let selected = |version| transitions.contains(&version);
        let mut changed: Vec<bool> = self
            .lines
            .iter()
            .map(|line| line.born > 0 && selected(line.born) || line.dies.is_some_and(selected))
            .collect();
        let versions = transitions.start.max(1)..transitions.end.min(self.version_count());
        for version in
            versions.filter(|&version| self.trailing_newline_changed(version) == Some(true))
        {
            let last = self
                .last_alive(version)
                .or_else(|| self.last_alive(version - 1));
            if let Some(flag) = last.and_then(|index| changed.get_mut(index)) {
                *flag = true;
            }
        }
        let mut hunks: Vec<Range<usize>> = Vec::new();

        for (index, _) in changed
            .iter()
            .enumerate()
            .filter(|&(_, &is_changed)| is_changed)
        {
            let start = index.saturating_sub(context);
            let end = (index + 1).saturating_add(context).min(self.lines.len());
            if let Some(last) = hunks.last_mut().filter(|last| start <= last.end) {
                last.end = end;
            } else {
                hunks.push(start..end);
            }
        }

        hunks
    }

    fn last_alive(&self, version: usize) -> Option<usize> {
        self.lines
            .iter()
            .rposition(|line| line.line_number(version).is_some())
    }
}

/// A line with a contiguous lifetime in the version chain.
#[derive(Debug, PartialEq, Eq)]
pub struct DisplayLine<'a> {
    text: &'a str,
    born: usize,
    dies: Option<usize>,
    line_numbers: Vec<u32>,
}

impl<'a> DisplayLine<'a> {
    /// Returns the borrowed line text with the line terminator stripped.
    pub fn text(&self) -> &'a str {
        self.text
    }

    /// Returns the first version containing the line.
    pub fn born(&self) -> usize {
        self.born
    }

    /// Returns the version in which the line was removed, if any.
    pub fn dies(&self) -> Option<usize> {
        self.dies
    }

    /// Returns the one-based line number in `version`, if the line is alive there.
    pub fn line_number(&self, version: usize) -> Option<u32> {
        self.line_numbers
            .get(version.checked_sub(self.born)?)
            .copied()
    }
}

/// The origin of a display line in a base, HEAD, and worktree stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreeWayChange {
    /// Present in all three versions.
    Context,
    /// Added in HEAD and still present in the worktree.
    CommittedAdd,
    /// Present in the base and removed in HEAD.
    CommittedDelete,
    /// Added in the worktree.
    UncommittedAdd,
    /// Present in the base and HEAD, then removed in the worktree.
    UncommittedDelete,
    /// Added in HEAD and removed in the worktree.
    CommittedThenRemoved,
}

/// Aligning a version would exceed the line limits of the underlying diff algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooManyLines {
    /// The zero-based index of the version that could not be aligned.
    pub version: usize,
}

impl fmt::Display for TooManyLines {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "aligning version {} exceeds the diff line limit",
            self.version
        )
    }
}

impl std::error::Error for TooManyLines {}

/// Aligns adjacent text versions using Histogram diff and the indent heuristic.
///
/// An insertion goes immediately before the next surviving line, or at the end
/// of the display if there is none. This puts it after all deletions in the gap,
/// including deletions from earlier versions.
///
/// Lines are split on `\n`. A trailing `\r` participates in comparison and is
/// stripped from the displayed text. A final `\n` is recorded separately, so a
/// change to that flag alone preserves the last line's lifetime. Empty text has
/// zero lines, and an empty version chain produces an empty stack.
///
/// # Errors
///
/// Returns [`TooManyLines`] if a version has `i32::MAX` or more lines or if an
/// adjacent pair has more than `u32::MAX / 2` distinct raw lines.
pub fn align<'a>(versions: &[&'a str]) -> Result<Stack<'a>, TooManyLines> {
    let Some((&base, later)) = versions.split_first() else {
        return Ok(Stack::default());
    };

    validate_line_count(base, 0)?;
    let mut order = DisplayOrder::default();
    let mut alive: Vec<_> = (1..)
        .zip(raw_lines(base))
        .map(|(number, raw)| order.insert_before(None, raw, 0, number))
        .collect();
    let mut next_alive = Vec::new();
    let mut diff = Diff::default();
    let mut before = base;

    for (version, &after) in (1..).zip(later) {
        validate_line_count(after, version)?;
        let mut input = InternedInput::default();
        input.update_before(raw_lines(before));
        input.update_after(raw_lines(after));
        let distinct_lines = input.interner.num_tokens();
        // Histogram allocates an occurrence pool with twice this capacity.
        if distinct_lines > u32::MAX / 2 {
            return Err(TooManyLines { version });
        }
        diff.compute_with(
            Algorithm::Histogram,
            &input.before,
            &input.after,
            distinct_lines,
        );
        diff.postprocess_lines(&input);

        order.apply(&diff, &mut alive, &mut next_alive, after, version);
        mem::swap(&mut alive, &mut next_alive);
        next_alive.clear();
        before = after;
    }

    Ok(Stack {
        lines: order.into_lines(),
        trailing_newlines: versions.iter().map(|text| text.ends_with('\n')).collect(),
    })
}

fn raw_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_terminator('\n')
}

fn validate_line_count(text: &str, version: usize) -> Result<(), TooManyLines> {
    let limit = i32::MAX as usize;
    if text.len() >= limit && raw_lines(text).take(limit).count() == limit {
        return Err(TooManyLines { version });
    }
    Ok(())
}

// Nodes are never removed. Live line IDs are kept separately so an adjacent
// diff only visits the lines in its two inputs. The predecessor links retain
// the ordering of deleted lines and let us insert directly before a survivor.
#[derive(Default)]
struct DisplayOrder<'a> {
    nodes: Vec<Node<'a>>,
    tail: Option<usize>,
}

struct Node<'a> {
    line: DisplayLine<'a>,
    previous: Option<usize>,
}

impl<'a> DisplayOrder<'a> {
    fn insert_before(
        &mut self,
        anchor: Option<usize>,
        raw: &'a str,
        born: usize,
        number: u32,
    ) -> usize {
        let id = self.nodes.len();
        // Every anchor is a live ID previously returned by this method.
        let previous = match anchor {
            Some(anchor) => self.nodes[anchor].previous.replace(id),
            None => self.tail.replace(id),
        };
        self.nodes.push(Node {
            line: DisplayLine {
                text: raw.strip_suffix('\r').unwrap_or(raw),
                born,
                dies: None,
                line_numbers: vec![number],
            },
            previous,
        });
        id
    }

    fn apply(
        &mut self,
        diff: &Diff,
        alive: &mut Vec<usize>,
        next_alive: &mut Vec<usize>,
        after: &'a str,
        version: usize,
    ) {
        let mut offset = 0;
        alive.retain(|&id| {
            // The live IDs correspond exactly to the before-side diff tokens.
            let removed = diff.is_removed(offset);
            offset += 1;
            if removed {
                self.nodes[id].line.dies = Some(version);
            }
            !removed
        });

        let mut survivor = 0;
        next_alive.extend((0..).zip(raw_lines(after)).map(|(offset, raw)| {
            let number = offset + 1;
            if diff.is_added(offset) {
                self.insert_before(alive.get(survivor).copied(), raw, version, number)
            } else {
                // Each non-added after token matches one retained ID, in order.
                let id = alive[survivor];
                survivor += 1;
                self.nodes[id].line.line_numbers.push(number);
                id
            }
        }));
    }

    fn into_lines(mut self) -> Vec<DisplayLine<'a>> {
        let mut lines = Vec::with_capacity(self.nodes.len());
        let mut previous = self.tail;
        while let Some(id) = previous {
            // The tail and all predecessor links refer to allocated nodes.
            let node = &mut self.nodes[id];
            previous = node.previous;
            lines.push(DisplayLine {
                line_numbers: mem::take(&mut node.line.line_numbers),
                ..node.line
            });
        }
        lines.reverse();
        lines
    }
}

#[cfg(test)]
mod tests;
