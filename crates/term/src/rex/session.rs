use std::{
    io,
    path::Path,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};

use super::{control::Control, socket};
use crate::Size;

const TERMINAL: &str = "com.superlogical.terminal";
const SHELL: &str = "com.superlogical.terminal.shell";
const WINDOW: &str = "layeredgit";

// Repository tabs may start on separate workers. Serialize discovery and creation within the app.
static DISCOVERY: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
struct Sessions {
    sessions: Vec<Session>,
}

#[derive(Deserialize)]
struct Session {
    session_id: String,
    label: Option<String>,
}

#[derive(Deserialize)]
struct View {
    label: Option<String>,
    windows: Vec<Window>,
}

#[derive(Deserialize)]
struct Attachment {
    view: View,
}

#[derive(Deserialize)]
struct Window {
    #[serde(rename = "window_id")]
    id: String,
    label: Option<String>,
    layers: Vec<Layer>,
}

#[derive(Deserialize)]
struct Layer {
    blocks: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    block_id: String,
    creator_name: String,
}

#[derive(Deserialize)]
pub(super) struct CreatedSession {
    pub(super) session_id: String,
    pub(super) initial_windows: Vec<CreatedWindow>,
}

#[derive(Deserialize)]
pub(super) struct CreatedWindow {
    pub(super) window_id: String,
    pub(super) block_ids: Vec<String>,
}

#[derive(Deserialize)]
struct TerminalProcess {
    child: Option<Process>,
    foreground: Option<Process>,
}

#[derive(Deserialize)]
struct Process {
    cwd: Option<String>,
}

pub(super) struct Target {
    pub(super) session_id: String,
    pub(super) window_id: String,
    pub(super) block_id: String,
    pub(super) label: String,
}

impl Window {
    fn terminals(&self) -> impl Iterator<Item = &Block> {
        self.layers
            .iter()
            .flat_map(|layer| &layer.blocks)
            .filter(|block| block.creator_name == TERMINAL)
    }
}

impl Target {
    pub(super) fn discover(
        control: &mut Control,
        cwd: &Path,
        size: Size,
        stopping: &AtomicBool,
    ) -> io::Result<Self> {
        let label = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| socket::invalid("Rex needs a UTF-8 repository folder name"))?;
        let layout = shell_layout(cwd, size)?;
        let _guard = DISCOVERY
            .lock()
            .map_err(|_| io::Error::other("Rex session discovery lock is poisoned"))?;
        let sessions: Sessions = control.call("session.list", &json!({}))?;
        let lower_label = label.to_lowercase();
        let named = sessions.sessions.iter().find(|session| {
            session
                .label
                .as_ref()
                .is_some_and(|name| name.to_lowercase() == lower_label)
        });
        let existing = if let Some(session) = named {
            Some((
                session.session_id.as_str(),
                attach(control, &session.session_id)?.view,
            ))
        } else {
            find_by_cwd(control, &sessions.sessions, cwd, stopping)
        };
        if stopping.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let Some((session_id, view)) = existing else {
            let created = create_session(control, label, &layout)?;
            let Attachment { view } = attach(control, &created.session_id)?;
            let window = created
                .initial_windows
                .into_iter()
                .next()
                .ok_or_else(|| socket::invalid("Rex did not create the initial window"))?;
            return Self::from_window(
                created.session_id,
                view.label.unwrap_or_else(|| label.to_owned()),
                window,
            );
        };
        let label = view.label.unwrap_or_else(|| label.to_owned());
        let session_id = session_id.to_owned();
        if let Some(window) = view
            .windows
            .iter()
            .find(|window| window.label.as_deref() == Some(WINDOW))
        {
            let block_id =
                if let Some(block) = window.terminals().next() {
                    block.block_id.clone()
                } else {
                    let created: CreatedBlocks = control.call("session.new_split", &json!({
                    "session_id": session_id, "window_id": window.id,
                    "direction": "horizontal", "side": "after", "focus": false, "layout": layout
                }))?;
                    first_block(created.block_ids)?
                };
            return Ok(Self {
                session_id,
                window_id: window.id.clone(),
                block_id,
                label,
            });
        }
        let window = control.call(
            "session.new_window",
            &json!({
                "session_id": session_id, "window_label": WINDOW, "focus": false, "layout": layout
            }),
        )?;
        Self::from_window(session_id, label, window)
    }

    fn from_window(session_id: String, label: String, window: CreatedWindow) -> io::Result<Self> {
        Ok(Self {
            session_id,
            window_id: window.window_id,
            block_id: first_block(window.block_ids)?,
            label,
        })
    }

    pub(super) fn terminal(
        &self,
        control: &mut Control,
        method: &str,
        args: &Value,
    ) -> io::Result<Value> {
        control.call(
            method,
            &terminal_args(&self.session_id, &self.block_id, args),
        )
    }

    pub(super) fn take_ownership(&self, control: &mut Control) -> io::Result<()> {
        control.call::<Value>(
            "session.own_block",
            &json!({
                "session_id": self.session_id,
                "block_ids": [self.block_id],
                "mode": "take",
            }),
        )?;
        Ok(())
    }

    pub(super) fn focus_event(&self, control: &mut Control, focused: bool) -> io::Result<()> {
        control.call::<Value>(
            "block.event",
            &json!({
                "session_id": self.session_id,
                "block_id": self.block_id,
                "event": {"type": "focus", "payload": {"action": if focused { "in" } else { "out" }}},
            }),
        )?;
        Ok(())
    }

    pub(super) fn show_in_rex(&self, control: &mut Control) -> io::Result<()> {
        // Opening an already running Rex.app brings it forward. If it was closed, wait
        // briefly for it to register its client actions.
        if !Command::new("/usr/bin/open")
            .args(["-b", "com.superlogical.rex"])
            .status()?
            .success()
        {
            return Err(io::Error::other("Could not open Rex.app"));
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let clients: Clients = control.call("client.list", &json!({"kinds": ["app"]}))?;
            if let Some(client) = clients.clients.into_iter().find(Client::is_rex_app) {
                control.call::<Value>(
                    "client.perform",
                    &json!({
                        "client_id": client.id,
                        "action": "session.select",
                        "args": {"session_id": self.session_id, "window_id": self.window_id},
                        "timeout_ms": 1500,
                    }),
                )?;
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(io::Error::other("No connected local Rex.app client"))
    }
}

#[derive(Deserialize)]
struct CreatedBlocks {
    block_ids: Vec<String>,
}

fn first_block(ids: Vec<String>) -> io::Result<String> {
    ids.into_iter()
        .next()
        .ok_or_else(|| socket::invalid("Rex did not create a terminal block"))
}

fn find_by_cwd<'a>(
    control: &mut Control,
    sessions: &'a [Session],
    cwd: &Path,
    stopping: &AtomicBool,
) -> Option<(&'a str, View)> {
    // Revision 16 requires attachment for process queries. Detach from sessions
    // that don't match. Bound discovery so an offline remote block can't stall startup.
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut remaining = 32;
    for session in sessions {
        if remaining == 0 || stopping.load(Ordering::Relaxed) || Instant::now() >= deadline {
            break;
        }
        let session_id = &session.session_id;
        let payload = json!({"session_id": session_id});
        let Ok(Attachment { view }) = control.call_before("session.attach", &payload, deadline)
        else {
            continue;
        };
        let mut matched = false;
        for block in view.windows.iter().flat_map(Window::terminals) {
            if remaining == 0 || stopping.load(Ordering::Relaxed) || Instant::now() >= deadline {
                break;
            }
            remaining -= 1;
            if terminal_in_repo(control, session_id, block, cwd, deadline) {
                matched = true;
                break;
            }
        }
        if matched {
            return Some((session_id, view));
        }
        let _ = control.call_before::<Value>("session.detach", &payload, deadline);
    }
    None
}

fn terminal_in_repo(
    control: &mut Control,
    session_id: &str,
    block: &Block,
    cwd: &Path,
    deadline: Instant,
) -> bool {
    let process: io::Result<TerminalProcess> = control.call_before(
        "com.superlogical.terminal.process",
        &terminal_args(session_id, &block.block_id, &json!({})),
        deadline,
    );
    process.is_ok_and(|process| {
        [process.child, process.foreground]
            .into_iter()
            .flatten()
            .filter_map(|process| process.cwd)
            .any(|path| Path::new(&path).starts_with(cwd))
    })
}

fn attach(control: &mut Control, session_id: &str) -> io::Result<Attachment> {
    control.call("session.attach", &json!({"session_id": session_id}))
}

pub(super) fn create_session(
    control: &mut Control,
    label: &str,
    layout: &Value,
) -> io::Result<CreatedSession> {
    control.call(
        "session.create",
        &json!({
            "label": label,
            "initial_windows": [{"window_label": WINDOW, "layout": layout}],
        }),
    )
}

pub(super) fn shell_layout(cwd: &Path, size: Size) -> io::Result<Value> {
    let cwd = cwd
        .to_str()
        .ok_or_else(|| socket::invalid("Rex needs a UTF-8 repository path"))?;
    Ok(json!({"block": {"flavor": SHELL, "options": {"cwd": cwd, "size": size_args(size)}}}))
}

pub(super) fn size_args(size: Size) -> Value {
    json!({
        "columns": size.cols, "rows": size.rows,
        "cell_width_px": size.cell_width, "cell_height_px": size.cell_height,
    })
}

fn terminal_args(session_id: &str, block_id: &str, args: &Value) -> Value {
    json!({"session_id": session_id, "block_id": block_id, "args": args})
}

#[derive(Deserialize)]
struct Clients {
    clients: Vec<Client>,
}

#[derive(Deserialize)]
struct Client {
    #[serde(rename = "client_id")]
    id: String,
    connection_state: Option<String>,
    info: Option<ClientInfo>,
    principal: Principal,
}

#[derive(Deserialize)]
struct ClientInfo {
    name: Option<String>,
    platform: Option<String>,
}

#[derive(Deserialize)]
struct Principal {
    transport: String,
}

impl Client {
    fn is_rex_app(&self) -> bool {
        self.principal.transport == "unix"
            && self.connection_state.as_deref() != Some("lost")
            && self.info.as_ref().is_some_and(|info| {
                info.platform.as_deref() == Some("macos")
                    && matches!(info.name.as_deref(), Some("Rex" | "Rex.app"))
            })
    }
}
