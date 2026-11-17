use std::ffi::OsStr;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use crate::Error;

pub(crate) fn command(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args([
            "--no-pager",
            "-c",
            "core.quotepath=off",
            "-c",
            "color.ui=never",
        ])
        .current_dir(directory)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    // An invoking hook or shell can carry the context of a different worktree.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
        "GIT_SHALLOW_FILE",
    ] {
        command.env_remove(variable);
    }
    command
}

pub(crate) fn run(command: &mut Command) -> Result<Output, Error> {
    command.output().map_err(spawn_error)
}

pub(crate) fn run_input(
    command: &mut Command,
    input: &[u8],
    operation: &'static str,
) -> Result<Output, Error> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_error)?;
    let mut stdin = child.stdin.take().ok_or(Error::Protocol(operation))?;
    thread::scope(|scope| {
        let writer = thread::Builder::new().spawn_scoped(scope, move || stdin.write_all(input));
        let output = child
            .wait_with_output()
            .map_err(|error| Error::io(operation, error));
        let written = writer
            .map_err(|error| Error::io(operation, error))?
            .join()
            .map_err(|_| Error::Protocol(operation))?;
        let output = output?;
        if output.status.success() {
            written.map_err(|error| Error::io(operation, error))?;
        }
        Ok(output)
    })
}

pub(crate) fn spawn_error(error: io::Error) -> Error {
    if error.kind() == io::ErrorKind::NotFound {
        Error::GitMissing
    } else {
        Error::io("start git", error)
    }
}

pub(crate) fn success(output: Output, operation: &'static str) -> Result<Vec<u8>, Error> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(Error::Git {
            operation,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

pub(crate) fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

pub(crate) fn read_path(directory: &Path, args: &[&str]) -> Result<PathBuf, Error> {
    let operation = "rev-parse repository path";
    let output = success(run(command(directory).args(args))?, operation)?;
    Ok(path_from_bytes(single_line(&output, operation)?))
}

pub(crate) fn single_line<'a>(bytes: &'a [u8], operation: &'static str) -> Result<&'a [u8], Error> {
    // A Git path itself may contain (or end in) a newline.
    bytes
        .strip_suffix(b"\n")
        .filter(|value| !value.is_empty())
        .ok_or(Error::Protocol(operation))
}

pub(crate) fn valid_oid(bytes: &[u8]) -> bool {
    matches!(bytes.len(), 40 | 64) && bytes.iter().all(u8::is_ascii_hexdigit)
}

pub(crate) fn oid(bytes: &[u8], operation: &'static str) -> Result<String, Error> {
    let value = single_line(bytes, operation)?;
    if !valid_oid(value) {
        return Err(Error::Protocol(operation));
    }
    String::from_utf8(value.to_vec()).map_err(|_| Error::Protocol(operation))
}

pub(crate) fn records<'a>(
    bytes: &'a [u8],
    operation: &'static str,
) -> Result<impl Iterator<Item = &'a [u8]>, Error> {
    if !bytes.is_empty() && !bytes.ends_with(b"\0") {
        return Err(Error::Protocol(operation));
    }
    Ok(bytes
        .split_inclusive(|&byte| byte == 0)
        .map(|record| record.strip_suffix(b"\0").unwrap_or(record)))
}
