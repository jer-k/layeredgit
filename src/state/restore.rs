//! Maps the selection, viewport anchor, and expanded folds across a new alignment.

use std::collections::{HashMap, HashSet};

use crate::model::{FileBody, FileDiff, Filter, Review, ReviewRow, ReviewRows, RowKind};

/// Prefer the latest raw path, then use a shared version path for a rename.
pub(super) fn match_files(old: &Review, new: &Review) -> Vec<Option<usize>> {
    let latest: HashMap<_, _> = new
        .files
        .iter()
        .enumerate()
        .filter_map(|(index, file)| file.paths().next_back().map(|path| (path, index)))
        .collect();
    let mut matches: Vec<_> = old
        .files
        .iter()
        .map(|file| {
            file.paths()
                .next_back()
                .and_then(|path| latest.get(path).copied())
        })
        .collect();
    let mut used: HashSet<_> = matches.iter().flatten().copied().collect();
    let mut aliases = HashMap::new();
    for (index, file) in new
        .files
        .iter()
        .enumerate()
        .filter(|(index, _)| !used.contains(index))
    {
        for path in file.paths() {
            aliases
                .entry(path)
                .and_modify(|existing| {
                    if *existing != Some(index) {
                        *existing = None;
                    }
                })
                .or_insert(Some(index));
        }
    }
    for (matched, file) in matches.iter_mut().zip(&old.files) {
        if matched.is_none() {
            *matched = file.paths().rev().find_map(|path| {
                aliases
                    .get(path)
                    .copied()
                    .flatten()
                    .filter(|&index| used.insert(index))
            });
        }
    }
    matches
}

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
        let (FileBody::Text(old_text), FileBody::Text(new_text)) = (&before.body, &after.body)
        else {
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
    let start = *rows.headers.get(file)?;
    let end = rows
        .headers
        .get(file + 1)
        .copied()
        .unwrap_or(rows.rows.len());
    let mut candidates = rows
        .rows
        .get(start..end)?
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
        | RowKind::Fold(std::ops::Range { start: index, .. }) => {
            let index = after.relocated_line(before, index)?;
            candidates
                .filter_map(|(i, row)| {
                    let distance = match &row.kind {
                        RowKind::Line(line) | RowKind::Hunk { start: line, .. } => {
                            index.abs_diff(*line)
                        }
                        RowKind::Fold(gap) => index.abs_diff(index.clamp(gap.start, gap.end - 1)),
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
