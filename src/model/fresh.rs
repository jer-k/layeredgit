//! Matches files across snapshots and aligns the old and new working text.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::input::ChangeKind;

use super::{FileBody, Filter, Review};

pub(crate) type FreshLines = HashMap<usize, HashSet<u32>>;

/// A new model with its old-to-new file mapping and newly born worktree lines.
pub(crate) struct Replacement {
    pub(crate) review: Arc<Review>,
    pub(crate) files: Vec<Option<usize>>,
    pub(crate) fresh: FreshLines,
    pub(crate) same_comparison: bool,
}

impl Replacement {
    pub(crate) fn new(review: Review, old: &Review) -> Self {
        let files = match_files(old, &review);
        let same_comparison = old.base_label == review.base_label
            && old.base_revision == review.base_revision
            && old.head_label == review.head_label;
        let fresh = if same_comparison {
            fresh_lines(old, &review, &files)
        } else {
            FreshLines::new()
        };
        Self {
            review: Arc::new(review),
            files,
            fresh,
            same_comparison,
        }
    }
}

fn fresh_lines(old: &Review, new: &Review, files: &[Option<usize>]) -> FreshLines {
    let previous: HashMap<_, _> = files
        .iter()
        .zip(&old.files)
        .filter_map(|(&index, file)| Some((index?, file)))
        .collect();
    new.files
        .iter()
        .enumerate()
        .filter_map(|(index, after)| {
            if after.uncommitted == ChangeKind::Unchanged
                || !matches!(after.body(Filter::All), FileBody::Text(_))
            {
                return None;
            }
            let worktree = after.worktree_text.as_deref()?;
            let before = match previous.get(&index) {
                Some(file) if file.paths.get(super::WORKTREE).is_some_and(Option::is_some) => {
                    file.worktree_text.as_deref()?
                }
                Some(_) => "",
                // Absent from the previous review means the file was clean, so its
                // working text matched HEAD. Only a file new to HEAD starts empty.
                None => after.head_text.as_deref().unwrap_or(""),
            };
            if before == worktree {
                return None;
            }
            let stack = diff_stack::align(&[before, worktree]).ok()?;
            let fresh: HashSet<_> = stack
                .lines()
                .iter()
                .filter(|line| line.born() == 1)
                .filter_map(|line| line.line_number(1))
                .collect();
            (!fresh.is_empty()).then_some((index, fresh))
        })
        .collect()
}

/// Relocates an active pulse if a reload left that file's working text intact.
pub(crate) fn relocate(
    fresh: FreshLines,
    old: &Review,
    new: &Review,
    files: &[Option<usize>],
) -> FreshLines {
    fresh
        .into_iter()
        .filter_map(|(index, lines)| {
            let after_index = files.get(index).copied().flatten()?;
            let before = old.files.get(index)?;
            let after = new.files.get(after_index)?;
            (after.uncommitted != ChangeKind::Unchanged
                && before.worktree_text == after.worktree_text)
                .then_some((after_index, lines))
        })
        .collect()
}

/// Prefer the latest raw path, then use a shared version path for a rename.
fn match_files(old: &Review, new: &Review) -> Vec<Option<usize>> {
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
