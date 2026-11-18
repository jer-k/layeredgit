//! Direct executable launches on macOS need a handoff in addition to Rune's
//! single-instance API, which delegates macOS launches to Launch Services.

#[cfg(target_os = "macos")]
pub(crate) use mac::claim;

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        fs::{self, OpenOptions},
        io::{self, ErrorKind, Read, Write},
        net::Shutdown,
        os::unix::{
            ffi::{OsStrExt, OsStringExt},
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        process::Command,
        thread,
        time::Duration,
    };

    use crate::{
        APP_ID,
        cli::{Launch, Options},
    };

    const ACK: u8 = b'A';
    const MAX_ARGS: u64 = 64 * 1024;
    const TIMEOUT: Duration = Duration::from_secs(2);

    pub(crate) fn claim(
        request: &Launch,
        dispatch: impl Fn(Launch) -> bool + Send + 'static,
    ) -> io::Result<bool> {
        // The OS assigns this private directory per user. Look it up directly so a
        // shell's custom TMPDIR still reaches a process launched from Finder.
        let temp = Command::new("/usr/bin/getconf")
            .arg("DARWIN_USER_TEMP_DIR")
            .output()?;
        let directory = PathBuf::from(std::ffi::OsString::from_vec(
            temp.stdout
                .strip_suffix(b"\n")
                .unwrap_or(&temp.stdout)
                .to_vec(),
        ));
        if !temp.status.success() || !directory.is_absolute() {
            return Err(io::Error::other(
                "cannot find the macOS user temporary directory",
            ));
        }
        claim_at(&directory, request, dispatch)
    }

    fn claim_at(
        directory: &Path,
        request: &Launch,
        dispatch: impl Fn(Launch) -> bool + Send + 'static,
    ) -> io::Result<bool> {
        let path = directory.join(format!("{APP_ID}.sock"));
        // Serialize connecting, reclaiming a dead process's socket, and binding.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join(format!("{APP_ID}.lock")))?;
        lock.lock()?;

        match UnixStream::connect(&path) {
            Ok(mut stream) => return forward(&mut stream, request).map(|()| false),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => fs::remove_file(&path)?,
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        thread::Builder::new()
            .name("review-launch".into())
            .spawn(move || {
                for mut stream in listener.incoming().map_while(Result::ok) {
                    if let Ok(request) = receive(&mut stream)
                        && dispatch(request)
                    {
                        let _ = stream.write_all(&[ACK]);
                    }
                }
            })?;
        Ok(true)
    }

    fn forward(stream: &mut UnixStream, request: &Launch) -> io::Result<()> {
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        if let Launch::Review(repository) = request {
            stream.write_all(b"--repo=")?;
            stream.write_all(repository.path.as_os_str().as_bytes())?;
            stream.write_all(b"\0")?;
            if let Some(base) = &repository.base {
                stream.write_all(b"--base=")?;
                stream.write_all(base.as_bytes())?;
                stream.write_all(b"\0")?;
            }
        }
        stream.shutdown(Shutdown::Write)?;
        let mut response = [0];
        stream.read_exact(&mut response)?;
        if response == [ACK] {
            Ok(())
        } else {
            Err(io::Error::other(
                "the running Git Viewer did not accept the launch",
            ))
        }
    }

    fn receive(stream: &mut UnixStream) -> io::Result<Launch> {
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut message = Vec::new();
        stream.take(MAX_ARGS + 1).read_to_end(&mut message)?;
        if message.len() as u64 > MAX_ARGS || !message.is_empty() && !message.ends_with(b"\0") {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "invalid launch message",
            ));
        }
        let args = message
            .split(|&byte| byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| std::ffi::OsString::from_vec(arg.to_vec()));
        let options =
            Options::parse(args).map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        if options.tooling.is_empty() && matches!(options.launch, Launch::Start | Launch::Review(_))
        {
            Ok(options.launch)
        } else {
            Err(io::Error::new(
                ErrorKind::InvalidData,
                "unexpected forwarded arguments",
            ))
        }
    }

    #[cfg(test)]
    mod tests {
        use std::{ffi::OsString, sync::mpsc};

        use super::*;
        use crate::source::Repository;

        #[test]
        #[allow(
            clippy::unwrap_used,
            reason = "A temporary socket and test messages are valid"
        )]
        fn later_invocation_delivers_exact_args_to_the_original_listener() {
            let directory = tempfile::tempdir().unwrap();
            let (send, receive) = mpsc::channel();
            assert!(
                claim_at(directory.path(), &Launch::Start, move |request| send
                    .send(request)
                    .is_ok())
                .unwrap()
            );
            let request = Launch::Review(Repository {
                path: PathBuf::from(OsString::from_vec(b"/repo with spaces/path=\xff".to_vec())),
                base: Some("-branch=previous".into()),
            });
            assert!(!claim_at(directory.path(), &request, |_| false).unwrap());
            assert_eq!(receive.recv_timeout(TIMEOUT).unwrap(), request);
            assert!(!claim_at(directory.path(), &Launch::Start, |_| false).unwrap());
            assert_eq!(receive.recv_timeout(TIMEOUT).unwrap(), Launch::Start);
        }
    }
}
