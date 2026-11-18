//! Maps the selection, viewport anchor, and expanded folds across a new alignment.

use std::collections::HashSet;

use crate::model::{FileBody, FileDiff, Filter, Review, ReviewRow, ReviewRows, RowKind};

pub(super) fn folds(
    old: &Review,
    new: &Review,
    matches: &[Option<usize>],
    expanded: &HashSet<(usize, usize, usize)>,
) -> HashSet<(usize, usize, usize)> {
    let mut restored = HashSet::new();
    for &(file, start, end) in expanded {
        let (Some(before), Some(index)) =
            (old.files.get(file), matches.get(file).copied().flatten())
        else {
            continue;
        };
        let Some(after) = new.files.get(index) else {
            continue;
        };
        let Some((first, last)) = end.checked_sub(1).and_then(|last| {
            after
                .relocated_line(before, start)
                .zip(after.relocated_line(before, last))
        }) else {
            continue;
        };
        for filter in Filter::ALL {
            let (FileBody::Text(old_text), FileBody::Text(new_text)) =
                (before.body(filter), after.body(filter))
            else {
                continue;
            };
            if old_text.gaps(filter).any(|gap| gap == (start..end)) {
                restored.extend(
                    new_text
                        .gaps(filter)
                        .filter(|gap| gap.start <= first.max(last) && first.min(last) < gap.end)
                        .map(|gap| (index, gap.start, gap.end)),
                );
            }
        }
    }
    restored
}

/// Finds the closest visible row within the matching file.
pub(super) fn scroll_row<'a>(
    rows: &'a ReviewRows,
    file: usize,
    old_row: &ReviewRow,
    before: &FileDiff,
    after: &FileDiff,
) -> Option<(usize, &'a ReviewRow)> {
    let range = rows.file_rows_range(file)?;
    let start = range.start;
    let mut candidates = rows
        .rows
        .get(range)?
        .iter()
        .enumerate()
        .map(|(i, row)| (start + i, row));
    match old_row.kind {
        RowKind::Header => candidates.next(),
        RowKind::Space => candidates.next_back(),
        RowKind::Placeholder(_) => {
            candidates.find(|(_, row)| matches!(row.kind, RowKind::Placeholder(_)))
        }
        RowKind::Newline(label) => candidates
            .find(|(_, row)| matches!(row.kind, RowKind::Newline(candidate) if label == candidate)),
        RowKind::Line(index)
        | RowKind::Hunk { start: index, .. }
        | RowKind::Fold {
            range: std::ops::Range { start: index, .. },
            ..
        } => {
            let index = after.relocated_line(before, index)?;
            candidates
                .filter_map(|(i, row)| {
                    let distance = match &row.kind {
                        RowKind::Line(line) | RowKind::Hunk { start: line, .. } => {
                            index.abs_diff(*line)
                        }
                        RowKind::Fold { range: gap, .. } => {
                            index.abs_diff(index.clamp(gap.start, gap.end - 1))
                        }
                        _ => return None,
                    };
                    let different_kind =
                        std::mem::discriminant(&row.kind) != std::mem::discriminant(&old_row.kind);
                    Some(((distance, different_kind), (i, row)))
                })
                .min_by_key(|(rank, _)| *rank)
                .map(|(_, row)| row)
        }
    }
}
