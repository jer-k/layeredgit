use std::{
    collections::{HashSet, hash_map::DefaultHasher},
    fs,
    hash::{Hash, Hasher},
    io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

use crate::{Error, Repo, git, validate_path};

/// An opaque snapshot of Git status and the metadata of its dirty paths and index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChangeToken(u64);

impl Repo {
    /// Computes a polling token from NUL-delimited status and filesystem metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read status or file metadata is inaccessible.
    pub fn change_token(&self) -> Result<ChangeToken, Error> {
        let operation = "status";
        let output = git::run(git::command(&self.root).args([
            operation,
            "--porcelain=v2",
            "-z",
            "--branch",
            "--no-ahead-behind",
            "--no-show-stash",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--no-renames",
        ]))?;
        let status = git::success(output, operation)?;
        let mut hasher = DefaultHasher::new();
        status.hash(&mut hasher);
        for record in git::records(&status, operation)? {
            // Porcelain v2 fixes the number of fields before the path. With
            // renames off there are no associated continuation records.
            let fields = match record.first() {
                Some(b'1') => 9,
                Some(b'u') => 11,
                Some(b'?') => 2,
                Some(b'#') => continue,
                _ => return Err(Error::Protocol(operation)),
            };
            let path = record
                .splitn(fields, |&byte| byte == b' ')
                .nth(fields - 1)
                .map(|path| path.strip_suffix(b"/").unwrap_or(path))
                .map(git::path_from_bytes)
                .ok_or(Error::Protocol(operation))?;
            validate_path(&path)?;
            hash_metadata(&self.root.join(path), &mut hasher)?;
        }
        hash_metadata(&self.index_path, &mut hasher)?;
        Ok(ChangeToken(hasher.finish()))
    }

    /// Checks a batch of repository-relative paths against Git's ignore rules.
    /// Tracked paths are kept even if an ignore rule also matches them.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid path or a failed Git invocation.
    pub fn ignored_paths<'a>(
        &self,
        paths: impl IntoIterator<Item = &'a Path>,
    ) -> Result<HashSet<PathBuf>, Error> {
        let mut input = Vec::new();
        for path in paths {
            validate_path(path)?;
            input.extend_from_slice(path.as_os_str().as_bytes());
            input.push(0);
        }
        if input.is_empty() {
            return Ok(HashSet::new());
        }
        let operation = "check-ignore";
        let output = git::run_input(
            git::command(&self.root).args([operation, "-z", "--stdin"]),
            &input,
            operation,
        )?;
        if output.status.code() == Some(1) {
            return Ok(HashSet::new());
        }
        let ignored = git::success(output, operation)?;
        Ok(git::records(&ignored, operation)?
            .map(git::path_from_bytes)
            .collect())
    }
}

fn hash_metadata(path: &Path, hasher: &mut impl Hasher) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Some((
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            None
        }
        Err(error) => return Err(Error::io("stat Git status path or index", error)),
    }
    .hash(hasher);
    Ok(())
}
