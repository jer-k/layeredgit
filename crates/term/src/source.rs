use std::sync::{
    Arc,
    mpsc::{SyncSender, TryRecvError},
};

use crate::Size;

/// Messages from a terminal byte source, in stream order.
#[derive(Debug)]
pub enum SourceEvent {
    /// The program started and can receive input.
    Started,
    /// A Rex shell attached, with its session label and a Ghostty snapshot of its screen and history.
    RexAttached {
        /// The label Rex displays for the session.
        session_label: String,
        /// Ghostty snapshot bytes, before any subsequent live output.
        snapshot: Vec<u8>,
    },
    /// The size Rex applied to the shared terminal.
    Resized(Size),
    /// A non-fatal error from a terminal action.
    Notice(String),
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

pub(super) type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(super) struct Output {
    events: SyncSender<SourceEvent>,
    wake: Wake,
}

impl Output {
    pub(super) fn new(events: SyncSender<SourceEvent>, wake: Wake) -> Self {
        Self { events, wake }
    }

    pub(super) fn send(&self, event: SourceEvent) -> bool {
        if self.events.send(event).is_err() {
            return false;
        }
        (self.wake)();
        true
    }
}
