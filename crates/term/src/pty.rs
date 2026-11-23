use std::{
    env, io,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
    },
    thread,
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::Size;

/// Messages from a terminal byte source, in stream order.
#[derive(Debug)]
pub enum SourceEvent {
    /// The program started and can receive input.
    Started,
    /// Bytes emitted by the program.
    Output(Vec<u8>),
    /// The session ended, with a human-readable exit status.
    Exited(String),
    /// The session failed, with its error message.
    Failed(String),
}

/// A bidirectional terminal byte stream. Calls from the view must not block on I/O.
pub trait Source {
    /// Queues bytes for the program's input.
    fn write(&self, data: Vec<u8>);

    /// Queues a resize for the program.
    fn resize(&self, size: Size);

    /// Takes the next available event without waiting.
    ///
    /// # Errors
    /// Reports whether the stream currently has no events or has disconnected.
    fn try_event(&self) -> Result<SourceEvent, TryRecvError>;
}

/// A local PTY session. Dropping it asks the worker to end the session.
pub struct LocalPty {
    commands: Sender<Command>,
    events: Receiver<SourceEvent>,
}

enum Command {
    Write(Vec<u8>),
    Resize(Size),
    Eof(io::Result<()>),
    Stop,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct Output {
    events: SyncSender<SourceEvent>,
    wake: Wake,
}

impl Output {
    fn send(&self, event: SourceEvent) -> bool {
        if self.events.send(event).is_err() {
            return false;
        }
        (self.wake)();
        true
    }
}

impl LocalPty {
    /// Starts the user's interactive login shell in `cwd`, on a worker thread.
    ///
    /// # Errors
    /// Returns an error if the worker thread cannot be started. PTY and spawn
    /// failures are delivered as events.
    pub fn login_shell(cwd: PathBuf, size: Size, wake: Wake) -> io::Result<Self> {
        Self::start(size, wake, move || {
            let mut command = CommandBuilder::new(
                env::var_os("SHELL")
                    .filter(|shell| !shell.is_empty())
                    .unwrap_or_else(|| "/bin/sh".into()),
            );
            command.args(["-l", "-i"]);
            command.cwd(cwd);
            command.env("TERM", ghostty_vt::terminfo::TERM);
            command.env("COLORTERM", "truecolor");
            command.env("TERMINFO", ghostty_vt::terminfo::dir()?);
            command.env(
                "GHOSTTY_RESOURCES_DIR",
                ghostty_vt::shell_integration::resources_dir()?,
            );
            Ok(command)
        })
    }

    fn start(
        size: Size,
        wake: Wake,
        command: impl FnOnce() -> io::Result<CommandBuilder> + Send + 'static,
    ) -> io::Result<Self> {
        let (commands, input) = mpsc::channel();
        let (send, events) = mpsc::sync_channel(32);
        let output = Output { events: send, wake };
        let reader_commands = commands.clone();
        thread::Builder::new()
            .name("terminal-pty".into())
            .spawn(move || {
                let result = command()
                    .and_then(|command| run(command, size, &input, reader_commands, &output));
                if let Err(error) = result {
                    output.send(SourceEvent::Failed(error.to_string()));
                }
            })?;
        Ok(Self { commands, events })
    }
}

impl Source for LocalPty {
    fn write(&self, data: Vec<u8>) {
        if !data.is_empty() {
            let _ = self.commands.send(Command::Write(data));
        }
    }

    fn resize(&self, size: Size) {
        let _ = self.commands.send(Command::Resize(size));
    }

    fn try_event(&self) -> Result<SourceEvent, TryRecvError> {
        self.events.try_recv()
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
    }
}

fn run(
    command: CommandBuilder,
    size: Size,
    input: &Receiver<Command>,
    reader_commands: Sender<Command>,
    output: &Output,
) -> io::Result<()> {
    let pair = native_pty_system()
        .openpty(size.into())
        .map_err(io::Error::other)?;
    let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
    let mut writer = pair.master.take_writer().map_err(io::Error::other)?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(io::Error::other)?;
    drop(pair.slave);
    let read_output = output.clone();
    let reader = thread::Builder::new()
        .name("terminal-read".into())
        .spawn(move || {
            let result = if read_output.send(SourceEvent::Started) {
                read(reader, &read_output)
            } else {
                Err(io::ErrorKind::BrokenPipe.into())
            };
            let _ = reader_commands.send(Command::Eof(result));
        });

    let mut failure = reader.err();
    if failure.is_none() {
        while let Ok(command) = input.recv() {
            match command {
                Command::Write(bytes) => {
                    // A child can close the PTY with keystrokes still queued. Let the
                    // reader report EOF and the exit status in order after its output.
                    let _ = writer.write_all(&bytes);
                }
                Command::Resize(size) => {
                    let _ = pair.master.resize(size.into());
                }
                Command::Eof(Ok(())) => {
                    let status = child.wait()?;
                    let reason = status
                        .signal()
                        .map_or_else(|| format!("code {}", status.exit_code()), str::to_owned);
                    output.send(SourceEvent::Exited(reason));
                    return Ok(());
                }
                Command::Eof(Err(error)) => {
                    failure = Some(error);
                    break;
                }
                Command::Stop => break,
            }
        }
    }

    let _ = child.kill();
    drop(writer);
    drop(pair.master);
    let reaped = child.wait().map(|_| ());
    failure.map_or(reaped, Err)
}

fn read(mut reader: impl Read, output: &Output) -> io::Result<()> {
    let mut buf = [0; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(len) => {
                if !output.send(SourceEvent::Output(buf[..len].to_vec())) {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

impl From<Size> for PtySize {
    fn from(size: Size) -> Self {
        Self {
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.cols.saturating_mul(size.cell_width),
            pixel_height: size.rows.saturating_mul(size.cell_height),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "Fail the test when PTY setup fails")]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn reads_output_before_exit() {
        let pty = LocalPty::start(
            Size {
                cols: 80,
                rows: 24,
                cell_width: 7,
                cell_height: 20,
            },
            Arc::new(|| {}),
            || {
                let mut command = CommandBuilder::new("/bin/sh");
                command.args(["-c", "printf 'hello from the PTY'; exit 7"]);
                Ok(command)
            },
        )
        .unwrap();
        assert!(matches!(
            pty.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            SourceEvent::Started
        ));
        let mut bytes = Vec::new();
        loop {
            match pty.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                SourceEvent::Output(data) => bytes.extend(data),
                SourceEvent::Exited(status) => {
                    assert_eq!(status, "code 7");
                    break;
                }
                SourceEvent::Failed(error) => panic!("{error}"),
                SourceEvent::Started => panic!("duplicate start event"),
            }
        }
        assert_eq!(bytes, b"hello from the PTY");
    }
}
