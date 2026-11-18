use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::{BINARY_PREFIX, Error, Repo, Revisions, git, has_binary_prefix, optionally};

/// The kind of a file at one layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file, possibly executable.
    Regular,
    /// A symbolic link whose contents are its target.
    Symlink,
    /// A Git submodule (gitlink), or an untracked embedded repository.
    Submodule,
}

/// The name and kind of a file in one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    /// Path relative to the working-tree root.
    pub path: PathBuf,
    /// Kind of entry at this layer.
    pub kind: FileKind,
}

/// How a file changed between two versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// No change in this comparison.
    None,
    /// A new file.
    Added,
    /// A removed file.
    Deleted,
    /// An edit, executable-bit change, or replacement at the same path.
    Modified,
    /// A rename detected by Git, possibly including edits.
    Renamed,
    /// The path changed between a regular file, symlink, or submodule.
    TypeChanged,
}

/// The number of lines added and deleted in one comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCounts {
    /// Lines added.
    pub added: u64,
    /// Lines deleted.
    pub deleted: u64,
}

/// How one diff layer changed a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    /// Kind of change.
    pub kind: ChangeKind,
    /// Git reported a binary diff, or the untracked file looks binary under its diff attributes.
    pub binary: bool,
    /// Both entries exist and have different Git modes.
    pub mode_changed: bool,
    /// Text line counts, when available.
    pub lines: Option<LineCounts>,
}

impl Change {
    const NONE: Self = Self {
        kind: ChangeKind::None,
        binary: false,
        mode_changed: false,
        lines: None,
    };
}

/// One file across the merge-base, HEAD, and working-tree versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// File in the merge-base, or absent.
    pub base: Option<FileVersion>,
    /// File in HEAD, or absent.
    pub head: Option<FileVersion>,
    /// File in the working tree, or absent.
    pub worktree: Option<FileVersion>,
    /// Merge-base-to-HEAD change.
    pub committed: Change,
    /// HEAD-to-working-tree change, including staged and untracked changes.
    pub uncommitted: Change,
}

impl ChangedFile {
    /// Returns the latest existing path, starting at the working-tree layer.
    pub fn path(&self) -> Option<&Path> {
        self.worktree
            .as_ref()
            .or(self.head.as_ref())
            .or(self.base.as_ref())
            .map(|version| version.path.as_path())
    }

    /// Returns whether either layer reported a binary diff.
    pub fn is_binary(&self) -> bool {
        self.committed.binary || self.uncommitted.binary
    }
}

struct Delta {
    before: Option<FileVersion>,
    after: Option<FileVersion>,
    change: Change,
}

pub(crate) fn changed_files(repo: &Repo, revisions: &Revisions) -> Result<Vec<ChangedFile>, Error> {
    let (committed, head_tree) =
        if let (Some(base), Some(head)) = (revisions.merge_base(), revisions.head()) {
            (diff(&repo.root, &[base, head])?, head.to_owned())
        } else {
            // Git recognizes the empty tree ID even when it has not been written
            // to the object database, including in SHA-256 repositories.
            let operation = "hash-object empty tree";
            let output =
                git::run(git::command(&repo.root).args(["hash-object", "-t", "tree", "--stdin"]))?;
            (
                Vec::new(),
                git::oid(&git::success(output, operation)?, operation)?,
            )
        };
    let working = diff(&repo.root, &[&head_tree])?;
    Ok(join_layers(committed, working, untracked(repo)?))
}

fn join_layers(
    committed: Vec<Delta>,
    working: Vec<Delta>,
    untracked: Vec<Delta>,
) -> Vec<ChangedFile> {
    let mut existing = BTreeMap::new();
    let mut added = BTreeMap::new();
    for delta in working {
        if let Some(before) = &delta.before {
            existing.insert(before.path.clone(), delta);
        } else if let Some(after) = &delta.after {
            added.insert(after.path.clone(), delta);
        }
    }
    for delta in untracked {
        let Some(after) = &delta.after else { continue };
        if let Some(previous) = existing
            .get_mut(&after.path)
            .filter(|previous| previous.change.kind == ChangeKind::Deleted)
        {
            // A staged removal can leave a newly untracked file at the same path.
            // The two Git reports are separate diffs and their counts cannot be added.
            previous.change = Change {
                kind: if previous
                    .before
                    .as_ref()
                    .is_some_and(|before| before.kind == after.kind)
                {
                    ChangeKind::Modified
                } else {
                    ChangeKind::TypeChanged
                },
                binary: previous.change.binary || delta.change.binary,
                lines: None,
                ..Change::NONE
            };
            previous.after = delta.after;
        } else {
            added.entry(after.path.clone()).or_insert(delta);
        }
    }

    let mut files = Vec::new();
    for Delta {
        before: base,
        after: head,
        change,
    } in committed
    {
        let working = if let Some(head) = &head {
            existing.remove(&head.path)
        } else {
            // A file deleted in HEAD may have been recreated at its original path.
            base.as_ref().and_then(|base| added.remove(&base.path))
        };
        let (worktree, uncommitted) = working.map_or_else(
            || (head.clone(), Change::NONE),
            |delta| (delta.after, delta.change),
        );
        files.push(ChangedFile {
            base,
            head,
            worktree,
            committed: change,
            uncommitted,
        });
    }
    files.extend(
        existing
            .into_values()
            .chain(added.into_values())
            .map(|delta| ChangedFile {
                base: delta.before.clone(),
                head: delta.before,
                worktree: delta.after,
                committed: Change::NONE,
                uncommitted: delta.change,
            }),
    );
    files.sort_by(|left, right| left.path().cmp(&right.path()));
    files
}

const DIFF: &str = "diff --raw --numstat";

fn diff(root: &Path, revisions: &[&str]) -> Result<Vec<Delta>, Error> {
    let output = git::run(
        git::command(root)
            .args([
                "diff",
                "--raw",
                "--numstat",
                "-z",
                "-M",
                "-l0",
                "-O/dev/null",
                "--no-ext-diff",
                "--no-textconv",
                "--no-relative",
                "--ignore-submodules=none",
            ])
            .args(revisions)
            .arg("--"),
    )?;
    parse_diff(&git::success(output, DIFF)?)
}

fn parse_diff(output: &[u8]) -> Result<Vec<Delta>, Error> {
    let mut records = git::records(output, DIFF)?.peekable();
    let mut changes = Vec::new();
    // With both formats selected, Git emits all raw records first, and then
    // one numstat record per raw record in the same order.
    while let Some(raw) = records.next_if(|record| record.starts_with(b":")) {
        let mut fields = raw.split(|&byte| byte == b' ');
        let (Some(old), Some(new), Some(_), Some(_), Some(status), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(Error::Protocol(DIFF));
        };
        let old = old.strip_prefix(b":").ok_or(Error::Protocol(DIFF))?;
        let kind = match status.first() {
            Some(b'A') => ChangeKind::Added,
            Some(b'D') => ChangeKind::Deleted,
            Some(b'M') => ChangeKind::Modified,
            Some(b'R') => ChangeKind::Renamed,
            Some(b'T') => ChangeKind::TypeChanged,
            _ => return Err(Error::Protocol(DIFF)),
        };
        let old_path = record_path(&mut records)?;
        let new_path = if kind == ChangeKind::Renamed {
            record_path(&mut records)?
        } else {
            old_path
        };
        changes.push(Delta {
            before: version(old, old_path)?,
            after: version(new, new_path)?,
            change: Change {
                kind,
                mode_changed: old != b"000000" && new != b"000000" && old != new,
                ..Change::NONE
            },
        });
    }
    for delta in &mut changes {
        read_numstat(&mut records, delta)?;
    }
    if records.next().is_some() {
        return Err(Error::Protocol(DIFF));
    }
    Ok(changes)
}

fn read_numstat<'a>(
    records: &mut impl Iterator<Item = &'a [u8]>,
    delta: &mut Delta,
) -> Result<(), Error> {
    let mut fields = records
        .next()
        .ok_or(Error::Protocol(DIFF))?
        .splitn(3, |&byte| byte == b'\t');
    let (Some(added), Some(deleted), Some(path)) = (fields.next(), fields.next(), fields.next())
    else {
        return Err(Error::Protocol(DIFF));
    };
    let path_matches = |version: Option<&FileVersion>, path| {
        version.is_some_and(|version| version.path.as_os_str().as_bytes() == path)
    };
    let valid_path = if delta.change.kind == ChangeKind::Renamed {
        path.is_empty()
            && path_matches(delta.before.as_ref(), record_path(records)?)
            && path_matches(delta.after.as_ref(), record_path(records)?)
    } else {
        path_matches(delta.after.as_ref().or(delta.before.as_ref()), path)
    };
    if !valid_path {
        return Err(Error::Protocol(DIFF));
    }
    if (added, deleted) == (b"-", b"-") {
        delta.change.binary = true;
    } else {
        delta.change.lines = Some(LineCounts {
            added: line_count(added)?,
            deleted: line_count(deleted)?,
        });
    }
    Ok(())
}

fn record_path<'a>(records: &mut impl Iterator<Item = &'a [u8]>) -> Result<&'a [u8], Error> {
    records
        .next()
        .filter(|path| !path.is_empty())
        .ok_or(Error::Protocol(DIFF))
}

fn line_count(bytes: &[u8]) -> Result<u64, Error> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|number| number.parse().ok())
        .ok_or(Error::Protocol(DIFF))
}

fn version(mode: &[u8], path: &[u8]) -> Result<Option<FileVersion>, Error> {
    let kind = match mode {
        b"000000" => return Ok(None),
        b"100644" | b"100755" => FileKind::Regular,
        b"120000" => FileKind::Symlink,
        b"160000" => FileKind::Submodule,
        _ => return Err(Error::Protocol(DIFF)),
    };
    Ok(Some(FileVersion {
        path: git::path_from_bytes(path),
        kind,
    }))
}

fn untracked(repo: &Repo) -> Result<Vec<Delta>, Error> {
    let operation = "ls-files --others";
    let output = git::run(git::command(&repo.root).args([
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ]))?;
    let paths = git::success(output, operation)?;
    let mut files = Vec::new();
    if paths.is_empty() {
        return Ok(files);
    }
    let mut attrs = diff_attributes(&repo.root, &paths)?;
    let big_file_threshold = big_file_threshold(&repo.root)?;
    for path in git::records(&paths, operation)? {
        let attribute = attrs.remove(path).flatten();
        if let Some(directory) = path.strip_suffix(b"/") {
            files.push(untracked_delta(
                git::path_from_bytes(directory),
                FileKind::Submodule,
                false,
                None,
            ));
        } else if let Some(delta) = untracked_file(
            repo,
            git::path_from_bytes(path),
            attribute,
            big_file_threshold,
        )? {
            files.push(delta);
        }
    }
    Ok(files)
}

fn big_file_threshold(root: &Path) -> Result<u64, Error> {
    let operation = "config core.bigFileThreshold";
    let output = git::run(git::command(root).args([
        "config",
        "--null",
        "--type=int",
        "--default",
        "512m",
        "--get",
        "core.bigFileThreshold",
    ]))?;
    let output = git::success(output, operation)?;
    std::str::from_utf8(
        output
            .strip_suffix(b"\0")
            .ok_or(Error::Protocol(operation))?,
    )
    .ok()
    .and_then(|value| value.parse().ok())
    .ok_or(Error::Protocol(operation))
}

fn diff_attributes(root: &Path, paths: &[u8]) -> Result<HashMap<Vec<u8>, Option<bool>>, Error> {
    let operation = "check-attr diff";
    let output = git::run_input(
        git::command(root).args(["check-attr", "-z", "--stdin", "diff"]),
        paths,
        operation,
    )?;
    let output = git::success(output, operation)?;
    let mut attrs = HashMap::new();
    let mut drivers = None;
    let mut records = git::records(&output, operation)?;
    while let Some(path) = records.next() {
        let (Some(b"diff"), Some(attribute)) = (records.next(), records.next()) else {
            return Err(Error::Protocol(operation));
        };
        let binary = match attribute {
            b"set" => Some(false),
            b"unset" => Some(true),
            b"unspecified" => None,
            name => {
                if drivers.is_none() {
                    drivers = Some(binary_drivers(root)?);
                }
                drivers
                    .as_ref()
                    .and_then(|drivers| drivers.get(name).copied())
            }
        };
        attrs.insert(path.to_vec(), binary);
    }
    Ok(attrs)
}

fn binary_drivers(root: &Path) -> Result<HashMap<Vec<u8>, bool>, Error> {
    let operation = "config diff drivers";
    let output = git::run(git::command(root).args([
        "config",
        "--type=bool-or-str",
        "--null",
        "--get-regexp",
        r"^diff\..*\.binary$",
    ]))?;
    if output.status.code() == Some(1) && output.stderr.is_empty() {
        return Ok(HashMap::new());
    }
    let output = git::success(output, operation)?;
    let mut drivers = HashMap::new();
    for record in git::records(&output, operation)? {
        let mut fields = record.splitn(2, |&byte| byte == b'\n');
        let (Some(key), Some(value)) = (fields.next(), fields.next()) else {
            return Err(Error::Protocol(operation));
        };
        let name = key
            .strip_prefix(b"diff.")
            .and_then(|key| key.strip_suffix(b".binary"))
            .ok_or(Error::Protocol(operation))?;
        match value {
            b"true" | b"false" => {
                drivers.insert(name.to_vec(), value == b"true");
            }
            b"auto" => {
                drivers.remove(name);
            }
            _ => return Err(Error::Protocol(operation)),
        }
    }
    Ok(drivers)
}

fn untracked_file(
    repo: &Repo,
    path: PathBuf,
    binary: Option<bool>,
    big_file_threshold: u64,
) -> Result<Option<Delta>, Error> {
    let Some((full_path, metadata)) = repo.worktree_metadata(&path)? else {
        return Ok(None);
    };
    let kind = if metadata.is_symlink() {
        FileKind::Symlink
    } else if metadata.is_file() {
        FileKind::Regular
    } else {
        return Ok(None);
    };
    if binary.unwrap_or(metadata.len() > big_file_threshold) {
        return Ok(Some(untracked_delta(path, kind, true, None)));
    }

    let bytes = if kind == FileKind::Symlink {
        let Some(target) = optionally(fs::read_link(full_path), "read untracked symlink")? else {
            return Ok(None);
        };
        target.into_os_string().into_vec()
    } else {
        let Some(file) = optionally(File::open(full_path), "open untracked file")? else {
            return Ok(None);
        };
        // Bound listing I/O. Only small text files get untracked line counts.
        let mut bytes = Vec::new();
        file.take(BINARY_PREFIX as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| Error::io("read untracked file prefix", error))?;
        bytes
    };
    let binary = binary.unwrap_or_else(|| has_binary_prefix(&bytes));
    let lines = (!binary && bytes.len() <= BINARY_PREFIX).then(|| LineCounts {
        added: bytes.split_inclusive(|&byte| byte == b'\n').count() as u64,
        deleted: 0,
    });
    Ok(Some(untracked_delta(path, kind, binary, lines)))
}

fn untracked_delta(
    path: PathBuf,
    kind: FileKind,
    binary: bool,
    lines: Option<LineCounts>,
) -> Delta {
    Delta {
        before: None,
        after: Some(FileVersion { path, kind }),
        change: Change {
            kind: ChangeKind::Added,
            binary,
            lines,
            ..Change::NONE
        },
    }
}
