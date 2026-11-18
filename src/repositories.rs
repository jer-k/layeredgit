//! Sidebar data and the serial worker for discovery, status and preferences.

use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, RecvTimeoutError, Sender},
    time::{Duration, Instant},
};

use repo::{Branch, Repo};
use rune::{AppHandle, PointerPress, Waker, prefs::Prefs};
use rune_kit::ComboboxState;

use crate::{
    cli::Launch, panels::Widths, preferences::Preferences, source::Repository, state::State,
};

const STATUS_INTERVAL: Duration = Duration::from_secs(30);

pub(crate) struct Workspace {
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: Option<usize>,
    pub(crate) collapsed: bool,
    pub(crate) pending: bool,
    pub(crate) add_error: Option<String>,
    pub(crate) menu: Option<(PathBuf, PointerPress)>,
    pub(crate) picker: [ComboboxState; 2],
    pub(crate) observed_picker: Option<Side>,
    pub(crate) waker: Option<Waker>,
    pub(crate) window: Visibility,
    pub(crate) sender: Option<Sender<Request>>,
    pub(crate) demo_pulse: bool,
    pub(crate) serial: u64,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            collapsed: false,
            pending: false,
            add_error: None,
            menu: None,
            picker: Default::default(),
            observed_picker: None,
            waker: None,
            window: Visibility::Shown,
            sender: None,
            demo_pulse: false,
            serial: 0,
        }
    }
}

pub(crate) enum Visibility {
    Shown,
    Hidden,
}

impl Workspace {
    pub(crate) fn live(sender: Sender<Request>) -> Self {
        Self {
            sender: Some(sender),
            ..Self::default()
        }
    }

    pub(crate) fn active(&self) -> Option<&Entry> {
        self.entries.get(self.selected?)
    }

    pub(crate) fn preferences(&self) -> Preferences {
        Preferences {
            repositories: self
                .entries
                .iter()
                .map(|entry| (entry.repository.path.clone(), entry.repository.base.clone()))
                .collect(),
            selected: self.active().map(|entry| entry.repository.path.clone()),
            collapsed: self.collapsed,
        }
    }

    pub(crate) fn send(&self, request: Request) -> bool {
        self.sender
            .as_ref()
            .is_some_and(|sender| sender.send(request).is_ok())
    }
}

pub(crate) struct Entry {
    pub(crate) repository: Repository,
    pub(crate) name: String,
    pub(crate) summary: Summary,
    pub(crate) branches: Vec<Branch>,
    pub(crate) head: Option<OsString>,
    pub(crate) validating: Option<u64>,
    pub(crate) revision_error: Option<String>,
    pub(crate) reload: bool,
    /// An inactive review with no workspace shell or live watcher attached.
    pub(crate) document: Option<Box<State>>,
}

impl Entry {
    pub(crate) fn new(repository: Repository) -> Self {
        let name = crate::desktop::repo_name(&repository.path);
        Self {
            repository,
            name,
            summary: Summary::default(),
            branches: Vec::new(),
            head: None,
            validating: None,
            revision_error: None,
            reload: false,
            document: None,
        }
    }
}

#[derive(Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) branch: String,
    pub(crate) dirty: bool,
}

impl Summary {
    fn read(repo: &Repo) -> Self {
        Self {
            branch: repo.head_name().map_or_else(
                |_| "Unavailable".into(),
                |branch| branch.to_string_lossy().into_owned(),
            ),
            dirty: repo.change_token().is_ok_and(repo::ChangeToken::is_dirty),
        }
    }
}

pub(crate) struct Discovered {
    pub(crate) repository: Repository,
    pub(crate) summary: Summary,
    pub(crate) branches: Vec<Branch>,
}

pub(crate) fn discover(mut repository: Repository) -> Result<Discovered, String> {
    let repo = Repo::open(&repository.path).map_err(|error| error.to_string())?;
    repository.path = repo
        .root()
        .canonicalize()
        .unwrap_or_else(|_| repo.root().to_owned());
    Ok(Discovered {
        repository,
        summary: Summary::read(&repo),
        branches: repo.branches().unwrap_or_default(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Base,
    Head,
}

impl Side {
    pub(crate) const fn index(self) -> usize {
        match self {
            Self::Base => 0,
            Self::Head => 1,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Base => "Base",
            Self::Head => "HEAD",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Choice {
    pub(crate) id: u64,
    pub(crate) side: Side,
    pub(crate) repository: Repository,
    pub(crate) head: Option<OsString>,
}

pub(crate) enum Request {
    Launch(Launch),
    Save(Preferences),
    ResizePanels(Widths),
    Inspect(PathBuf),
    Choose(Choice),
}

pub(crate) fn route(
    initial: Launch,
    requests: &Receiver<Request>,
    handle: &AppHandle<State>,
    mut prefs: Prefs,
) {
    let mut saved = Preferences::load(&prefs);
    let restored = saved.clone();
    let widths = Widths::load(&prefs);
    let select = initial == Launch::Start;
    handle.update(move |state, tx| {
        state.panels.preferred.set(widths);
        state.restore_workspace(restored, select, tx);
    });
    let mut log = std::env::var_os("LAYEREDGIT_LAUNCH_LOG").and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    launch(initial, handle, &mut log);
    let mut due = Instant::now();
    loop {
        match requests.recv_timeout(due.saturating_duration_since(Instant::now())) {
            Ok(Request::Launch(request)) => launch(request, handle, &mut log),
            Ok(Request::Save(settings)) => {
                if let Err(error) = settings.save(&mut prefs) {
                    let error = format!("Couldn’t save repositories: {error}");
                    handle.update(move |state, _| state.workspace_error(error));
                }
                saved = settings;
            }
            Ok(Request::ResizePanels(widths)) => {
                if let Err(error) = widths.save(&mut prefs) {
                    let error = format!("Couldn’t save panel widths: {error}");
                    handle.update(move |state, _| state.workspace_error(error));
                }
            }
            Ok(Request::Inspect(path)) => inspect(path, handle),
            Ok(Request::Choose(choice)) => {
                let result = choice
                    .repository
                    .validate(choice.head.as_deref())
                    .map_err(|error| error.to_string());
                handle.update(move |state, tx| state.finish_choice(choice, result, tx));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if Instant::now() >= due {
            refresh_summaries(&saved, handle);
            due = Instant::now() + STATUS_INTERVAL;
        }
    }
}

fn launch(request: Launch, handle: &AppHandle<State>, log: &mut Option<std::fs::File>) {
    if let Some(log) = log {
        let _ = writeln!(log, "pid={} request={request:?}", std::process::id());
    }
    match request {
        Launch::Review(repository) => {
            let result = discover(repository);
            handle.open();
            handle.update(move |state, tx| state.repository_resolved(result, tx));
        }
        Launch::Start => handle.open(),
        Launch::Demo => {}
    }
}

fn inspect(path: PathBuf, handle: &AppHandle<State>) {
    let Ok(repo) = Repo::open(&path) else { return };
    let summary = Summary::read(&repo);
    let branches = repo.branches().unwrap_or_default();
    handle.update_if(move |state, _| state.update_summary(&path, summary, Some(branches)));
}

fn refresh_summaries(saved: &Preferences, handle: &AppHandle<State>) {
    let summaries: Vec<_> = saved
        .repositories
        .iter()
        .map(|(path, _)| {
            let summary = Repo::open(path).map_or_else(
                |_| Summary {
                    branch: "Unavailable".into(),
                    dirty: false,
                },
                |repo| Summary::read(&repo),
            );
            (path.clone(), summary)
        })
        .collect();
    handle.update_if(move |state, _| {
        let mut changed = false;
        for (path, summary) in summaries {
            changed |= state.update_summary(&path, summary, None);
        }
        changed
    });
}

pub(crate) fn row_key(path: &Path) -> String {
    format!("repository/{}", crate::desktop::path_key(path))
}
