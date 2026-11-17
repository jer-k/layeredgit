//! Owned input for a review, independent of where the versions came from.

/// A branch comparison with working-tree changes layered on top.
pub(crate) struct ReviewInput {
    /// Label for the merge-base side of the comparison.
    pub base_label: String,
    /// Label for the checked-out branch or commit.
    pub head_label: String,
    /// Files in review order.
    pub files: Vec<InputFile>,
}

/// One logical file across the merge base, HEAD, and working tree.
pub(crate) struct InputFile {
    /// Version at the merge base, if the file exists there.
    pub base: Option<FileVersion>,
    /// Version at HEAD, if the file exists there.
    pub head: Option<FileVersion>,
    /// Version in the working tree, if the file exists there.
    pub worktree: Option<FileVersion>,
    /// Whether to show a binary placeholder instead of aligning text.
    pub binary: bool,
    /// Change from the merge base to HEAD.
    pub committed: ChangeKind,
    /// Change from HEAD to the working tree.
    pub uncommitted: ChangeKind,
}

/// The path and content of an existing version of a file.
pub(crate) struct FileVersion {
    /// Repository-relative path at this version.
    pub path: String,
    /// UTF-8 content. Binary versions can carry just the path.
    pub text: Option<String>,
}

/// The file-level change in one layer of the review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    /// No file change in this layer.
    Unchanged,
    /// The file contents or metadata changed.
    Modified,
    /// The file did not exist in the previous version.
    Added,
    /// The file no longer exists.
    Deleted,
    /// The file moved to a different path, possibly with content changes.
    Renamed,
}
