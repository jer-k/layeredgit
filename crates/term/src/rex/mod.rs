mod apc;
mod control;
mod data;
mod session;
mod snapshot;
mod socket;

use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    },
    thread,
    time::Duration,
};

use serde::Deserialize;
use serde_json::json;

use self::{
    apc::Parser,
    control::Control,
    data::{Data, Frame},
    session::{Target, size_args},
};
use crate::{
    Size, Source, SourceEvent,
    source::{Output, Wake},
};

/// An attachment to a persistent Rex terminal block.
///
/// Dropping the source disconnects the client. Rex retains the session and terminal.
pub struct RexPty {
    commands: Sender<Command>,
    events: Receiver<SourceEvent>,
    stopping: Arc<AtomicBool>,
}

enum Command {
    Write(Vec<u8>),
    Resize(Size),
    ShowInRex,
}

impl RexPty {
    /// Resolves or creates the repository's Rex terminal on a worker thread.
    ///
    /// # Errors
    /// Returns an error if the worker cannot start. Server, discovery and stream
    /// failures are delivered as events. A successful attach emits `RexAttached`.
    pub fn connect(cwd: PathBuf, size: Size, wake: Wake) -> io::Result<Self> {
        let (commands, input) = mpsc::channel();
        let (send, events) = mpsc::sync_channel(32);
        let output = Output::new(send, wake);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stopping);
        thread::Builder::new()
            .name("terminal-rex-control".into())
            .spawn(move || {
                if let Err(error) = run(&cwd, size, &input, &output, &worker_stop)
                    && !worker_stop.load(Ordering::Relaxed)
                {
                    output.send(SourceEvent::Failed(error.to_string()));
                }
            })?;
        Ok(Self {
            commands,
            events,
            stopping,
        })
    }

    /// Queues an action to bring Rex.app forward and display the attached window.
    pub fn open_in_rex(&self) {
        let _ = self.commands.send(Command::ShowInRex);
    }
}

impl Source for RexPty {
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

impl Drop for RexPty {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
    }
}

#[derive(Deserialize)]
struct Ticket {
    ticket: String,
}

fn run(
    cwd: &std::path::Path,
    size: Size,
    input: &Receiver<Command>,
    output: &Output,
    stopping: &Arc<AtomicBool>,
) -> io::Result<()> {
    let endpoint = socket::endpoint()?;
    let mut control = Control::connect(&endpoint)?;
    let target = Target::discover(&mut control, cwd, size, stopping)?;
    target.terminal(
        &mut control,
        "com.superlogical.terminal.resize",
        &size_args(size),
    )?;
    let ticket: Ticket = control.call(
        "data.connect",
        &json!({
            "session_id": target.session_id,
            "block_id": target.block_id,
            "data_ref_id": "terminal",
            "subprotocol": data::PROTOCOL,
        }),
    )?;
    let mut data = Data::connect(endpoint, &ticket.ticket)?;
    let (snapshot, remainder) = data.read_snapshot(stopping)?;
    let output_reader = output.clone();
    let stopping_reader = Arc::clone(stopping);
    let label = target.label.clone();
    thread::Builder::new()
        .name("terminal-rex-data".into())
        .spawn(move || {
            if output_reader.send(SourceEvent::RexAttached {
                session_label: label,
                snapshot,
            }) && let Err(error) =
                read_output(data, &remainder, &output_reader, &stopping_reader)
                && !stopping_reader.load(Ordering::Relaxed)
            {
                output_reader.send(SourceEvent::Failed(error.to_string()));
            }
        })?;
    while !stopping.load(Ordering::Relaxed) {
        match input.recv_timeout(Duration::from_millis(500)) {
            Ok(command) => run_command(command, &mut control, &target, output)?,
            Err(RecvTimeoutError::Timeout) => control.idle()?,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

fn run_command(
    command: Command,
    control: &mut Control,
    target: &Target,
    output: &Output,
) -> io::Result<()> {
    match command {
        Command::Write(bytes) => match std::str::from_utf8(&bytes) {
            Ok(mut text) => {
                // Leave room for the control envelope and JSON escaping of input.
                while !text.is_empty() {
                    let (part, rest) = text.split_at(text.floor_char_boundary(64 * 1024));
                    target.terminal(
                        control,
                        "com.superlogical.terminal.write",
                        &json!({"data": part}),
                    )?;
                    text = rest;
                }
            }
            Err(_) => {
                output.send(SourceEvent::Notice(
                    "Rex terminal input must be UTF-8".into(),
                ));
            }
        },
        Command::Resize(size) => {
            target.terminal(
                control,
                "com.superlogical.terminal.resize",
                &size_args(size),
            )?;
        }
        Command::ShowInRex => {
            if let Err(error) = target.show_in_rex(control) {
                output.send(SourceEvent::Notice(error.to_string()));
            }
        }
    }
    Ok(())
}

fn read_output(
    mut data: Data,
    remainder: &[u8],
    output: &Output,
    stopping: &AtomicBool,
) -> io::Result<()> {
    let mut parser = Parser::new(data.nonce());
    if !emit(&mut parser, remainder, output) {
        return Ok(());
    }
    while !stopping.load(Ordering::Relaxed) {
        match data.read(stopping) {
            Ok(Frame::Idle) => {}
            Ok(Frame::Output(bytes)) => {
                if !emit(&mut parser, &bytes, output) {
                    return Ok(());
                }
            }
            end => {
                if let Some(event) = parser.flush()
                    && !output.send(event)
                {
                    return Ok(());
                }
                if let Frame::Closed(reason) = end? {
                    output.send(SourceEvent::Exited(reason.into()));
                }
                return Ok(());
            }
        }
    }
    Ok(())
}

fn emit(parser: &mut Parser, bytes: &[u8], output: &Output) -> bool {
    for event in parser.push(bytes) {
        let exited = matches!(event, SourceEvent::Exited(_));
        if !output.send(event) || exited {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests;
