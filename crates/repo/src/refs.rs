use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
};

use crate::{Error, Repo, git};

/// A local or remote-tracking branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// Branch name without `refs/heads/` or `refs/remotes/`.
    pub name: OsString,
    /// Full ref name, unambiguous even when a tag shares the short name.
    pub reference: OsString,
    /// Full upstream ref, if configured.
    pub upstream: Option<OsString>,
    /// Tip object ID.
    pub tip: String,
    /// Whether this is the branch checked out in the repository's own worktree.
    pub is_current: bool,
    /// Whether this is a remote-tracking branch.
    pub is_remote: bool,
}

/// A checkout registered with Git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute worktree path.
    pub path: PathBuf,
    /// Full ref of the checked-out local branch, or absent when detached or bare.
    pub branch: Option<OsString>,
    /// Whether Git considers the checkout stale and eligible for pruning.
    pub prunable: bool,
}

impl Repo {
    /// Lists local and remote-tracking branches, excluding remote symbolic aliases.
    ///
    /// # Errors
    ///
    /// Returns an error if Git fails or returns malformed output.
    pub fn branches(&self) -> Result<Vec<Branch>, Error> {
        let operation = "for-each-ref";
        let output = git::run(git::command(self.root()).args([
            operation,
            "--format=%(refname)%00%(upstream)%00%(objectname)%00%(HEAD)%00%(symref)",
            "refs/heads/",
            "refs/remotes/",
        ]))?;
        let output = git::success(output, operation)?;
        output
            .split(|&byte| byte == b'\n')
            .filter(|record| !record.is_empty())
            .filter_map(|record| parse_branch(record, operation).transpose())
            .collect()
    }

    /// Lists the repository's registered working trees.
    ///
    /// # Errors
    ///
    /// Returns an error if Git fails or returns malformed output.
    pub fn worktrees(&self) -> Result<Vec<Worktree>, Error> {
        let operation = "worktree list";
        let output =
            git::run(git::command(self.root()).args(["worktree", "list", "--porcelain", "-z"]))?;
        let output = git::success(output, operation)?;
        let mut trees = Vec::new();
        let mut current = None;
        for field in git::records(&output, operation)? {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                current = Some(Worktree {
                    path: git::path_from_bytes(path),
                    branch: None,
                    prunable: false,
                });
            } else if field.is_empty() {
                trees.extend(current.take());
            } else if let Some(tree) = &mut current {
                if let Some(branch) = field.strip_prefix(b"branch ") {
                    tree.branch = Some(OsString::from_vec(branch.to_vec()));
                }
                tree.prunable |= field == b"prunable" || field.starts_with(b"prunable ");
            }
        }
        trees.extend(current);
        Ok(trees)
    }

    /// Resolves a revision expression to a commit ID (peeling annotated tags).
    ///
    /// # Errors
    ///
    /// Returns an error if the revision does not name a commit or Git fails.
    pub fn resolve(&self, revision: &OsStr) -> Result<String, Error> {
        let mut expression = revision.to_owned();
        expression.push("^{commit}");
        let operation = "resolve revision";
        let output = git::run(
            git::command(self.root())
                .args(["rev-parse", "--verify", "--end-of-options"])
                .arg(expression),
        )?;
        if !output.status.success() && output.status.code() == Some(128) {
            return Err(Error::InvalidRevision(revision.to_owned()));
        }
        git::oid(&git::success(output, operation)?, operation)
    }

    pub(crate) fn worktree_for(&self, head: &OsStr) -> Result<Option<PathBuf>, Error> {
        let operation = "resolve symbolic revision";
        let output = git::run(
            git::command(self.root())
                .args([
                    "rev-parse",
                    "--symbolic-full-name",
                    "--verify",
                    "--end-of-options",
                ])
                .arg(head),
        )?;
        if output.status.code() == Some(128) {
            return Ok(None);
        }
        let output = git::success(output, operation)?;
        let name = output.strip_suffix(b"\n").unwrap_or(&output);
        if !name.starts_with(b"refs/heads/") {
            return Ok(None);
        }
        Ok(self.worktrees()?.into_iter().find_map(|tree| {
            (!tree.prunable && tree.branch.as_deref() == Some(OsStr::from_bytes(name)))
                .then_some(tree.path)
        }))
    }
}

fn parse_branch(record: &[u8], operation: &'static str) -> Result<Option<Branch>, Error> {
    let fields: Vec<_> = record.split(|&byte| byte == 0).collect();
    let [reference, upstream, tip, current, symbolic] = fields.as_slice() else {
        return Err(Error::Protocol(operation));
    };
    if !symbolic.is_empty() {
        return Ok(None);
    }
    let (name, is_remote) = reference
        .strip_prefix(b"refs/heads/")
        .map(|name| (name, false))
        .or_else(|| {
            reference
                .strip_prefix(b"refs/remotes/")
                .map(|name| (name, true))
        })
        .ok_or(Error::Protocol(operation))?;
    if !git::valid_oid(tip) {
        return Err(Error::Protocol(operation));
    }
    Ok(Some(Branch {
        name: OsString::from_vec(name.to_vec()),
        reference: OsString::from_vec(reference.to_vec()),
        upstream: (!upstream.is_empty()).then(|| OsString::from_vec(upstream.to_vec())),
        tip: String::from_utf8_lossy(tip).into_owned(),
        is_current: *current == b"*",
        is_remote,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used, reason = "A valid ref record must parse")]
    fn preserves_ref_bytes_without_requiring_utf8() {
        let branch = parse_branch(
            b"refs/heads/topic/odd-\xff\0\0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\0*\0",
            "for-each-ref",
        )
        .unwrap()
        .unwrap();
        assert_eq!(branch.name.as_bytes(), b"topic/odd-\xff");
        assert!(branch.is_current);
    }
}
