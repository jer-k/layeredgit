use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::thread::{self, JoinHandle};

use crate::{Contents, Error, git};

const OPERATION: &str = "cat-file --batch-command";

#[derive(Debug)]
pub(crate) struct Batch {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<Vec<u8>>>,
}

impl Batch {
    pub(crate) fn start(directory: &Path) -> Result<Self, Error> {
        let mut child = git::command(directory)
            .args(["cat-file", "--batch-command", "-Z"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(git::spawn_error)?;
        let stdin = child.stdin.take().ok_or(Error::Protocol(OPERATION))?;
        let stdout = child.stdout.take().ok_or(Error::Protocol(OPERATION))?;
        let mut stderr = child.stderr.take().ok_or(Error::Protocol(OPERATION))?;
        // Drain diagnostics concurrently so a failing command cannot block on stderr.
        let reader = thread::Builder::new().spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        let stderr = reader.map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            Error::io("start git cat-file stderr reader", error)
        })?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr: Some(stderr),
        })
    }

    pub(crate) fn read(&mut self, expression: &[u8], limit: u64) -> Result<Contents, Error> {
        let info = self.request(b"info ", expression)?;
        if info.strip_suffix(b" missing") == Some(expression) {
            return Ok(Contents::Missing);
        }
        let (oid, kind, size) = parse_info(&info)?;
        match kind {
            b"blob" => {}
            b"commit" => return Ok(Contents::Submodule),
            b"tree" => return Ok(Contents::Missing),
            _ => return Err(Error::Protocol(OPERATION)),
        }
        if size > limit {
            return Ok(Contents::TooLarge { size, limit });
        }

        // Use the resolved object ID for the second request so an index update
        // cannot change the selected blob between the size check and the read.
        let contents = self.request(b"contents ", oid)?;
        if contents != info {
            return Err(Error::Protocol(OPERATION));
        }
        let mut bytes = Vec::new();
        (&mut self.stdout)
            .take(size)
            .read_to_end(&mut bytes)
            .map_err(|error| Error::io("read git cat-file blob", error))?;
        let mut delimiter = [0];
        self.stdout
            .read_exact(&mut delimiter)
            .map_err(|error| Error::io("read git cat-file delimiter", error))?;
        if bytes.len() as u64 != size || delimiter != [0] {
            return Err(Error::Protocol(OPERATION));
        }
        Ok(Contents::Bytes(bytes))
    }

    fn request(&mut self, command: &[u8], expression: &[u8]) -> Result<Vec<u8>, Error> {
        self.stdin
            .write_all(command)
            .and_then(|()| self.stdin.write_all(expression))
            .and_then(|()| self.stdin.write_all(b"\0"))
            .map_err(|error| Error::io("write git cat-file request", error))?;
        let mut header = Vec::new();
        self.stdout
            .read_until(0, &mut header)
            .map_err(|error| Error::io("read git cat-file header", error))?;
        if header.pop() != Some(0) {
            return Err(Error::Protocol(OPERATION));
        }
        Ok(header)
    }

    pub(crate) fn failure(mut self, error: Error) -> Error {
        let status = self.child.try_wait().ok().flatten().or_else(|| {
            let _ = self.child.kill();
            self.child.wait().ok()
        });
        let stderr = self.take_stderr();
        match status {
            Some(status) if !status.success() && !stderr.is_empty() => Error::Git {
                operation: OPERATION,
                status,
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            },
            _ => error,
        }
    }

    fn take_stderr(&mut self) -> Vec<u8> {
        self.stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.take_stderr();
    }
}

fn parse_info(info: &[u8]) -> Result<(&[u8], &[u8], u64), Error> {
    let mut fields = info.split(|&byte| byte == b' ');
    let (Some(oid), Some(kind), Some(size), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(Error::Protocol(OPERATION));
    };
    if !git::valid_oid(oid) {
        return Err(Error::Protocol(OPERATION));
    }
    let size = std::str::from_utf8(size)
        .ok()
        .and_then(|size| size.parse().ok())
        .ok_or(Error::Protocol(OPERATION))?;
    Ok((oid, kind, size))
}
