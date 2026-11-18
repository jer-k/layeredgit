//! Reads the three Git versions used by the review.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use repo::{BinaryReason, ChangedFile, Contents, Error, Layer, Repo, Revisions, decode_text};

use crate::input::{ChangeKind, FileVersion, InputFile, Placeholder, ReviewInput};

mod refresh;
mod watch;
pub(crate) use refresh::{Refresh, RefreshEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Repository {
    pub(crate) path: PathBuf,
    pub(crate) base: Option<OsString>,
}

#[derive(PartialEq, Eq)]
struct Comparison {
    base: OsString,
    head_label: String,
    revisions: Revisions,
}

pub(crate) fn ref_label(reference: &OsStr) -> String {
    let label = reference.to_string_lossy();
    label
        .strip_prefix("refs/heads/")
        .or_else(|| label.strip_prefix("refs/remotes/"))
        .unwrap_or(&label)
        .to_owned()
}

impl Repository {
    /// Builds the input using the merge base, HEAD, and the live working tree.
    pub(crate) fn load_input(&self) -> Result<ReviewInput, Error> {
        self.preview(None).map(|(input, _)| input)
    }

    pub(crate) fn preview(&self, head: Option<&OsStr>) -> Result<(ReviewInput, bool), Error> {
        let repo = Repo::open(&self.path)?;
        let comparison = self.comparison(&repo, head)?;
        let worktree = comparison.revisions.worktree().is_some();
        Ok((Self::read_input(repo, &comparison)?, worktree))
    }

    pub(crate) fn validate(&self, head: Option<&OsStr>) -> Result<(), Error> {
        let repo = Repo::open(&self.path)?;
        let comparison = self.comparison(&repo, head)?;
        if comparison.revisions.head().is_none()
            && let Some(base) = &self.base
        {
            repo.resolve(base)?;
        }
        Ok(())
    }

    fn comparison(&self, repo: &Repo, head: Option<&OsStr>) -> Result<Comparison, Error> {
        let base = match &self.base {
            Some(base) => Some(base.clone()),
            None => match repo.default_base() {
                Ok(base) => Some(base),
                Err(Error::NoDefaultBase) => None,
                Err(error) => return Err(error),
            },
        };
        let revisions = repo.compare(base.as_deref(), head)?;
        let head_label = match head {
            Some(head) => ref_label(head),
            None => ref_label(&repo.head_name()?),
        };
        Ok(Comparison {
            base: base.unwrap_or_else(|| "Default".into()),
            head_label,
            revisions,
        })
    }

    fn read_input(mut repo: Repo, comparison: &Comparison) -> Result<ReviewInput, Error> {
        let Comparison {
            base,
            head_label,
            revisions,
        } = comparison;
        let files = repo
            .changed_files(revisions)?
            .into_iter()
            .map(|file| {
                let mut input = convert_file(&file, |layer| repo.read(revisions, &file, layer))?;
                if revisions.worktree().is_none() {
                    input.worktree.clone_from(&input.head);
                    input.placeholders[1] = None;
                }
                Ok::<_, Error>(input)
            })
            .collect::<Result<_, _>>()?;
        Ok(ReviewInput {
            base_label: ref_label(base),
            base_revision: [revisions.base_tip(), revisions.merge_base()]
                .map(|id| id.map(str::to_owned)),
            head_label: head_label.clone(),
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
