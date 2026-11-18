//! Reads the three Git versions used by the review.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use repo::{BinaryReason, ChangedFile, Contents, Error, Layer, Repo, Revisions, decode_text};

use crate::input::{ChangeKind, FileVersion, InputFile, Placeholder, ReviewInput};

mod refresh;
mod watch;
pub(crate) use refresh::Refresh;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Repository {
    pub(crate) path: PathBuf,
    pub(crate) base: Option<OsString>,
}

#[derive(PartialEq, Eq)]
struct Comparison {
    base: OsString,
    revisions: Revisions,
}

impl Repository {
    /// Builds the input using the merge base, HEAD, and the live working tree.
    pub(crate) fn load_input(&self) -> Result<ReviewInput, Error> {
        let repo = Repo::open(&self.path)?;
        let comparison = self.comparison(&repo)?;
        self.read_input(repo, &comparison)
    }

    fn comparison(&self, repo: &Repo) -> Result<Comparison, Error> {
        let base = match &self.base {
            Some(base) => base.clone(),
            None => repo.default_base()?,
        };
        let revisions = repo.revisions(Some(&base))?;
        Ok(Comparison { base, revisions })
    }

    fn read_input(&self, mut repo: Repo, comparison: &Comparison) -> Result<ReviewInput, Error> {
        let Comparison { base, revisions } = comparison;
        let base_label = base.to_string_lossy();
        let base_label = if self.base.is_some() {
            &base_label
        } else {
            base_label
                .strip_prefix("refs/heads/")
                .or_else(|| base_label.strip_prefix("refs/remotes/"))
                .unwrap_or(&base_label)
        };
        let head_label = repo.head_name()?.to_string_lossy().into_owned();
        let files = repo
            .changed_files(revisions)?
            .into_iter()
            .map(|file| convert_file(&file, |layer| repo.read(revisions, &file, layer)))
            .collect::<Result<_, _>>()?;
        Ok(ReviewInput {
            base_label: base_label.into(),
            base_revision: [revisions.base_tip(), revisions.merge_base()]
                .map(|id| id.map(str::to_owned)),
            head_label,
            files,
        })
    }
}

/// Reads only versions listed for this file. A worktree path can vanish after the listing.
fn convert_file<E>(
    file: &ChangedFile,
    mut read: impl FnMut(Layer) -> Result<Contents, E>,
) -> Result<InputFile, E> {
    let mut version = |layer, version: Option<&repo::FileVersion>| {
        let Some(version) = version else {
            return Ok((None, None));
        };
        Ok(convert_version(&version.path, read(layer)?))
    };
    let (base, base_reason) = version(Layer::Base, file.base.as_ref())?;
    let (head, head_reason) = version(Layer::Head, file.head.as_ref())?;
    let (worktree, worktree_reason) = version(Layer::Worktree, file.worktree.as_ref())?;
    let placeholder = |change: repo::Change, before, after| {
        [change.binary.then_some(Placeholder::Binary), before, after]
            .into_iter()
            .flatten()
            .max()
    };
    Ok(InputFile {
        base,
        head,
        worktree,
        placeholders: [
            placeholder(file.committed, base_reason, head_reason),
            placeholder(file.uncommitted, head_reason, worktree_reason),
        ],
        committed: change_kind(file.committed),
        uncommitted: change_kind(file.uncommitted),
    })
}

fn convert_version(path: &Path, contents: Contents) -> (Option<FileVersion>, Option<Placeholder>) {
    let text = match contents {
        Contents::Missing => return (None, None),
        Contents::Bytes(bytes) => match decode_text(&bytes) {
            Ok(text) => (Some(text.to_owned()), None),
            Err(BinaryReason::NulByte) => (None, Some(Placeholder::Binary)),
            Err(BinaryReason::InvalidUtf8) => (None, Some(Placeholder::NotUtf8)),
        },
        Contents::TooLarge { size, .. } => (None, Some(Placeholder::TooLarge(size))),
        Contents::Submodule => (None, Some(Placeholder::Submodule)),
    };
    let (text, reason) = text;
    (
        Some(FileVersion {
            path: path.to_owned(),
            text,
        }),
        reason,
    )
}

fn change_kind(change: repo::Change) -> ChangeKind {
    match change.kind {
        repo::ChangeKind::None => ChangeKind::Unchanged,
        repo::ChangeKind::Added => ChangeKind::Added,
        repo::ChangeKind::Deleted => ChangeKind::Deleted,
        repo::ChangeKind::TypeChanged => ChangeKind::ModeChanged,
        repo::ChangeKind::Modified if change.mode_changed => ChangeKind::ModeChanged,
        repo::ChangeKind::Modified => ChangeKind::Modified,
        repo::ChangeKind::Renamed => ChangeKind::Renamed,
    }
}

#[cfg(test)]
pub(crate) mod tests;
