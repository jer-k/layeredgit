//! Reads merge-base, HEAD, index, and working-tree versions from a local Git repo.
//!
//! Operations are blocking. Use Git 2.42 or newer for NUL-delimited batch reads.
//! Paths are relative to the repository root and preserve their original Unix bytes.

#![cfg(unix)]

mod batch;
mod diff;
mod git;

use std::collections::hash_map::DefaultHasher;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use batch::Batch;
use git::{command, oid, path_from_bytes, single_line, success};

pub use diff::{Change, ChangeKind, ChangedFile, FileKind, FileVersion, LineCounts};

/// Default maximum size of an individual file version, in bytes.
pub const DEFAULT_BYTE_LIMIT: u64 = 10 * 1024 * 1024;
const BINARY_PREFIX: usize = 8000;

/// A repository operation failed.
#[derive(Debug)]
pub enum Error {
    /// The Git executable could not be found on PATH.
    GitMissing,
    /// The starting path is outside a Git working tree.
    NotRepository {
        /// Path used for discovery.
        path: PathBuf,
        /// Git's diagnostic.
        stderr: String,
    },
    /// No origin/HEAD, main, or master ref could be found.
    NoDefaultBase,
    /// A file path was empty, absolute, or contained traversal or NUL bytes.
    InvalidPath(
        /// Invalid path.
        PathBuf,
    ),
    /// A Git subprocess failed.
    Git {
        /// Operation being performed.
        operation: &'static str,
        /// Subprocess exit status.
        status: ExitStatus,
        /// Git's diagnostic.
        stderr: String,
    },
    /// A filesystem or subprocess I/O operation failed.
    Io {
        /// Operation being performed.
        operation: &'static str,
        /// Underlying I/O error.
        source: io::Error,
    },
    /// Git returned data in an unexpected format.
    Protocol(
        /// Operation being performed.
        &'static str,
    ),
}

impl Error {
    fn io(operation: &'static str, source: io::Error) -> Self {
        Self::Io { operation, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitMissing => write!(formatter, "git executable not found on PATH"),
            Self::NotRepository { path, stderr } => {
                write!(
                    formatter,
                    "{} is outside a Git working tree",
                    path.display()
                )?;
                write_stderr(formatter, stderr)
            }
            Self::NoDefaultBase => write!(formatter, "no origin/HEAD, main, or master base ref"),
            Self::InvalidPath(path) => {
                write!(formatter, "invalid repository file path {}", path.display())
            }
            Self::Git {
                operation,
                status,
                stderr,
            } => {
                write!(formatter, "git {operation} failed ({status})")?;
                write_stderr(formatter, stderr)
            }
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::Protocol(operation) => {
                write!(formatter, "unexpected Git output from {operation}")
            }
        }
    }
}

fn write_stderr(formatter: &mut fmt::Formatter<'_>, stderr: &str) -> fmt::Result {
    if !stderr.trim().is_empty() {
        write!(formatter, ": {}", stderr.trim())?;
    }
    Ok(())
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The committed endpoints of a comparison, captured as object IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revisions {
    merge_base: Option<String>,
    head: Option<String>,
}

impl Revisions {
    /// Returns the merge-base commit ID, or `None` for an unborn HEAD.
    pub fn merge_base(&self) -> Option<&str> {
        self.merge_base.as_deref()
    }

    /// Returns the HEAD commit ID, or `None` for an unborn HEAD.
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }
}

/// One of the three file versions in a [`ChangedFile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The merge-base tree.
    Base,
    /// The HEAD tree.
    Head,
    /// The working tree on disk.
    Worktree,
}

/// The result of reading one file version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contents {
    /// No file at this layer.
    Missing,
    /// File bytes, or the raw target bytes for a symlink.
    Bytes(
        /// Contents of the file.
        Vec<u8>,
    ),
    /// The file exceeds the configured size limit.
    TooLarge {
        /// File size in bytes, or a lower bound if the file grew during the read.
        size: u64,
        /// Configured byte limit.
        limit: u64,
    },
    /// A Git submodule (gitlink).
    Submodule,
}

/// Why a file's bytes cannot be displayed as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryReason {
    /// A NUL byte occurs in the first 8000 bytes.
    NulByte,
    /// The bytes are not valid UTF-8.
    InvalidUtf8,
}

/// Borrows UTF-8 text from file bytes, without replacing invalid bytes.
///
/// # Errors
///
/// Returns [`BinaryReason::NulByte`] if the first 8000 bytes contain a NUL,
/// otherwise [`BinaryReason::InvalidUtf8`] if UTF-8 validation fails.
pub fn decode_text(bytes: &[u8]) -> Result<&str, BinaryReason> {
    if has_binary_prefix(bytes) {
        return Err(BinaryReason::NulByte);
    }
    std::str::from_utf8(bytes).map_err(|_| BinaryReason::InvalidUtf8)
}

fn has_binary_prefix(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_PREFIX).any(|&byte| byte == 0)
}

/// An opaque, equality-comparable snapshot of Git status and index metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChangeToken(u64);

/// A local Git working tree and its reusable blob reader.
#[derive(Debug)]
pub struct Repo {
    root: PathBuf,
    git_dir: PathBuf,
    index_path: PathBuf,
    byte_limit: u64,
    batch: Option<Batch>,
    batch_index: Option<IndexStamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexStamp {
    Missing,
    Present {
        inode: u64,
        ctime: i64,
        ctime_nsec: i64,
    },
}

impl Repo {
    /// Discovers the working tree containing a directory or an existing file.
    ///
    /// # Errors
    ///
    /// Returns an error if Git is unavailable, the path is outside a working
    /// tree, or Git cannot read the repository.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| Error::io("stat repository discovery path", error))?;
        let directory = if metadata.is_dir() || metadata.is_symlink() && path.is_dir() {
            path
        } else {
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
        };
        let operation = "rev-parse --show-toplevel";
        let output = git::run(command(directory).args(["rev-parse", "--show-toplevel"]))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success()
            && (stderr.contains("not a git repository")
                || stderr.contains("must be run in a work tree"))
        {
            return Err(Error::NotRepository {
                path: path.to_owned(),
                stderr: stderr.into_owned(),
            });
        }
        let root = path_from_bytes(single_line(&success(output, operation)?, operation)?);
        let git_dir = git::read_path(&root, &["rev-parse", "--absolute-git-dir"])?;
        let index_path = git::read_path(
            &root,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )?;
        Ok(Self {
            root,
            git_dir,
            index_path,
            byte_limit: DEFAULT_BYTE_LIMIT,
            batch: None,
            batch_index: None,
        })
    }

    /// Returns the absolute working-tree root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the absolute Git directory for this worktree.
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Sets the maximum size of an individual file read, in bytes.
    pub fn set_byte_limit(&mut self, limit: u64) {
        self.byte_limit = limit;
    }

    /// Returns the target of origin/HEAD, or the full ref of local main or master.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoDefaultBase`] if none of these refs exist, or an error
    /// if Git fails.
    pub fn default_base(&self) -> Result<OsString, Error> {
        if let Some(target) = self.symbolic_ref("refs/remotes/origin/HEAD")? {
            return Ok(target);
        }
        for candidate in ["refs/heads/main", "refs/heads/master"] {
            let output =
                git::run(command(&self.root).args(["show-ref", "--verify", "--quiet", candidate]))?;
            if output.status.success() {
                return Ok(candidate.into());
            }
            if output.status.code() != Some(1) {
                success(output, "show-ref")?;
            }
        }
        Err(Error::NoDefaultBase)
    }

    /// Returns the checked-out branch name, or Git's short object ID for a detached HEAD.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot resolve HEAD.
    pub fn head_name(&self) -> Result<OsString, Error> {
        if let Some(name) = self.symbolic_ref("HEAD")? {
            let full = name.as_bytes();
            return Ok(OsString::from_vec(
                full.strip_prefix(b"refs/heads/").unwrap_or(full).to_vec(),
            ));
        }
        let operation = "rev-parse --short HEAD";
        let output = git::run(command(&self.root).args(["rev-parse", "--short", "HEAD"]))?;
        Ok(OsString::from_vec(
            single_line(&success(output, operation)?, operation)?.to_vec(),
        ))
    }

    /// Resolves the supplied base ref (or the default) and computes its merge-base
    /// with HEAD. For an unborn HEAD, both endpoints are empty.
    ///
    /// # Errors
    ///
    /// Returns an error if the base is unavailable, no merge-base exists, or Git
    /// cannot resolve the commits.
    pub fn revisions(&self, base: Option<&OsStr>) -> Result<Revisions, Error> {
        let output = git::run(command(&self.root).args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            "HEAD^{commit}",
        ]))?;
        if output.status.code() == Some(1)
            && let Some(head_ref) = self.symbolic_ref("HEAD")?
        {
            let exists = git::run(
                command(&self.root)
                    .args(["show-ref", "--verify", "--quiet"])
                    .arg(head_ref),
            )?;
            if exists.status.code() == Some(1) && exists.stderr.is_empty() {
                return Ok(Revisions {
                    merge_base: None,
                    head: None,
                });
            }
            success(exists, "show-ref HEAD target")?;
        }
        let head = oid(&success(output, "resolve HEAD")?, "resolve HEAD")?;
        let mut base_rev = match base {
            Some(base) => base.to_owned(),
            None => self.default_base()?,
        };
        base_rev.push("^{commit}");
        let output = git::run(
            command(&self.root)
                .args(["rev-parse", "--verify", "--end-of-options"])
                .arg(base_rev),
        )?;
        let tip = oid(&success(output, "resolve base")?, "resolve base")?;
        let output = git::run(command(&self.root).args(["merge-base", &tip, &head]))?;
        let merge_base = oid(&success(output, "merge-base")?, "merge-base")?;
        Ok(Revisions {
            merge_base: Some(merge_base),
            head: Some(head),
        })
    }

    fn symbolic_ref(&self, name: &str) -> Result<Option<OsString>, Error> {
        let operation = "symbolic-ref";
        let output = git::run(command(&self.root).args([operation, "--quiet", name]))?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        Ok(Some(OsString::from_vec(
            single_line(&success(output, operation)?, operation)?.to_vec(),
        )))
    }

    /// Lists the union of committed, working-tree, and untracked changes, sorted
    /// by the latest existing path of each file.
    ///
    /// Pass revisions captured from this repository. The index and worktree are
    /// read live. Git detects renames among tracked and staged paths. A rename
    /// whose destination is still untracked appears as a deletion and an addition.
    ///
    /// # Errors
    ///
    /// Returns an error if Git or the filesystem cannot report the changes.
    pub fn changed_files(&self, revisions: &Revisions) -> Result<Vec<ChangedFile>, Error> {
        diff::changed_files(self, revisions)
    }

    /// Reads a changed file at its path in the selected layer.
    ///
    /// Pass the same revisions used to list the changed file. Worktree contents
    /// are read live and may have changed since the listing.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is invalid or Git or the filesystem fails.
    pub fn read(
        &mut self,
        revisions: &Revisions,
        file: &ChangedFile,
        layer: Layer,
    ) -> Result<Contents, Error> {
        let (version, revision) = match layer {
            Layer::Base => (file.base.as_ref(), revisions.merge_base()),
            Layer::Head => (file.head.as_ref(), revisions.head()),
            Layer::Worktree => (file.worktree.as_ref(), None),
        };
        let Some(version) = version else {
            return Ok(Contents::Missing);
        };
        if version.kind == FileKind::Submodule {
            return Ok(Contents::Submodule);
        }
        match revision {
            Some(revision) => {
                validate_path(&version.path)?;
                let mut expression = OsString::from(revision);
                expression.push(":");
                expression.push(&version.path);
                self.read_blob(expression.as_bytes())
            }
            None if layer == Layer::Worktree => self.read_worktree(&version.path),
            None => Ok(Contents::Missing),
        }
    }

    /// Reads the stage-zero index version at a repository-relative path.
    ///
    /// Use the staged destination path for a staged rename. Paths with only
    /// conflict stages are reported as missing.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is invalid or Git fails.
    pub fn read_index(&mut self, path: &Path) -> Result<Contents, Error> {
        validate_path(path)?;
        // cat-file caches the index after its first stage lookup.
        let stamp = optionally(fs::metadata(&self.index_path), "stat Git index")?.map_or(
            IndexStamp::Missing,
            |metadata| IndexStamp::Present {
                inode: metadata.ino(),
                ctime: metadata.ctime(),
                ctime_nsec: metadata.ctime_nsec(),
            },
        );
        if self.batch_index.is_some_and(|previous| previous != stamp) {
            self.batch = None;
        }
        self.batch_index = Some(stamp);
        let mut expression = OsString::from(":0:");
        expression.push(path);
        let contents = self.read_blob(expression.as_bytes())?;
        if contents != Contents::Missing {
            return Ok(contents);
        }

        // A gitlink's referenced commit usually lives outside this object database.
        let output = git::run(
            command(&self.root)
                .args(["--literal-pathspecs", "ls-files", "--stage", "-z", "--"])
                .arg(path),
        )?;
        let output = success(output, "ls-files --stage")?;
        for entry in git::records(&output, "ls-files --stage")? {
            let mut fields = entry.splitn(2, |&byte| byte == b'\t');
            if let (Some(fields), Some(name)) = (fields.next(), fields.next())
                && name == path.as_os_str().as_bytes()
                && fields.starts_with(b"160000 ")
                && fields.ends_with(b" 0")
            {
                return Ok(Contents::Submodule);
            }
        }
        Ok(Contents::Missing)
    }

    fn read_blob(&mut self, expression: &[u8]) -> Result<Contents, Error> {
        let mut batch = match self.batch.take() {
            Some(batch) => batch,
            None => Batch::start(&self.root)?,
        };
        let result = batch.read(expression, self.byte_limit);
        match result {
            Ok(contents) => {
                self.batch = Some(batch);
                Ok(contents)
            }
            Err(error) => Err(batch.failure(error)),
        }
    }

    /// Reads a repository-relative path directly from the working tree.
    ///
    /// Symlinks are read as their raw target bytes. Directories and special
    /// filesystem entries are reported as missing. Use [`Self::read`] with a
    /// changed file to identify submodules.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is invalid or a filesystem read fails.
    pub fn read_worktree(&self, path: &Path) -> Result<Contents, Error> {
        let Some((full_path, metadata)) = self.worktree_metadata(path)? else {
            return Ok(Contents::Missing);
        };
        let limit = self.byte_limit;
        let size = metadata.len();
        if metadata.is_symlink() {
            if size > limit {
                return Ok(Contents::TooLarge { size, limit });
            }
            let Some(target) = optionally(fs::read_link(full_path), "read working-tree symlink")?
            else {
                return Ok(Contents::Missing);
            };
            let bytes = target.into_os_string().into_vec();
            if bytes.len() as u64 > limit {
                return Ok(Contents::TooLarge {
                    size: bytes.len() as u64,
                    limit,
                });
            }
            return Ok(Contents::Bytes(bytes));
        }
        if !metadata.is_file() {
            return Ok(Contents::Missing);
        }
        if size > limit {
            return Ok(Contents::TooLarge { size, limit });
        }
        let Some(file) = optionally(File::open(full_path), "open working-tree file")? else {
            return Ok(Contents::Missing);
        };
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| Error::io("read working-tree file", error))?;
        let size = bytes.len() as u64;
        if size > limit {
            return Ok(Contents::TooLarge { size, limit });
        }
        Ok(Contents::Bytes(bytes))
    }

    fn worktree_metadata(&self, path: &Path) -> Result<Option<(PathBuf, fs::Metadata)>, Error> {
        validate_path(path)?;
        let mut full_path = self.root.clone();
        let mut components = path.components().peekable();
        while let Some(component) = components.next() {
            full_path.push(component);
            let Some(metadata) =
                optionally(fs::symlink_metadata(&full_path), "stat working-tree path")?
            else {
                return Ok(None);
            };
            if components.peek().is_none() {
                return Ok(Some((full_path, metadata)));
            }
            if !metadata.is_dir() {
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// Computes a polling token from porcelain status (including the HEAD OID)
    /// and the index file's modification time and size.
    ///
    /// It can miss further edits to an already-dirty or untracked file when its
    /// status stays the same. It does not track base-ref movement. Use file
    /// watching or explicit refreshes for those cases.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot read status or index metadata is inaccessible.
    pub fn change_token(&self) -> Result<ChangeToken, Error> {
        let output = git::run(command(&self.root).args([
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--no-ahead-behind",
            "--no-show-stash",
            "--untracked-files=all",
            "--ignore-submodules=none",
            "--no-renames",
        ]))?;
        let mut hasher = DefaultHasher::new();
        success(output, "status")?.hash(&mut hasher);
        if let Some(metadata) = optionally(fs::metadata(&self.index_path), "stat Git index")? {
            metadata.len().hash(&mut hasher);
            metadata
                .modified()
                .map_err(|error| Error::io("read Git index modification time", error))?
                .hash(&mut hasher);
        }
        Ok(ChangeToken(hasher.finish()))
    }
}

fn optionally<T>(result: io::Result<T>, operation: &'static str) -> Result<Option<T>, Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::io(operation, error)),
    }
}

fn validate_path(path: &Path) -> Result<(), Error> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0)
        || bytes
            .split(|&byte| byte == b'/')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
    {
        return Err(Error::InvalidPath(path.to_owned()));
    }
    Ok(())
}
