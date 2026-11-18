//! Own the start window, route launches by working-tree root, and create document windows.

use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use rune::{App, AppHandle, AppSpec, RunOptions, Titlebar, Tx, WindowSpec, prefs::Prefs};

use crate::{
    APP_ID, APP_NAME,
    cli::{Launch, Options},
    frame, recents,
    source::Repository,
    start::{self, Start},
    state::State,
    view,
};

type Reviews = Rc<RefCell<HashMap<PathBuf, AppHandle<State>>>>;

#[derive(Clone)]
pub(crate) struct Client {
    start: AppHandle<Start>,
    send: Sender<Launch>,
    reviews: Reviews,
}

impl Client {
    pub(crate) fn open(&self, path: PathBuf) -> bool {
        self.send
            .send(Launch::Review(Repository { path, base: None }))
            .is_ok()
    }

    pub(crate) fn show_start(&self) {
        self.start.open();
    }

    pub(crate) fn show_review(&self, repository: Repository, head: String) {
        let root = repository.path.clone();
        let review = self.reviews.borrow().get(&root).cloned();
        if let Some(review) = review {
            if let Some(base) = repository.base {
                review.update_if(move |state, tx| {
                    state.set_base(base, tx);
                    false
                });
            }
            review.open();
        } else {
            self.create_review(repository, head);
        }
        self.start.hide();
    }

    fn create_review(&self, repository: Repository, head: String) {
        let root = repository.path.clone();
        let session = ReviewSession {
            client: self.clone(),
            root: root.clone(),
            head,
        };
        let frame_name = format!("review:{}", path_key(&root));
        let size = frame::size(&frame_name, (1240.0, 820.0));
        let app = review_app(size, Some(repository), Some(session), false, false)
            .on_window(move |window| frame::restore(window, &frame_name))
            .on_close(State::close);
        self.reviews.borrow_mut().insert(root.clone(), app.handle());
        let native = rune::open_window(WindowSpec::from_app(path_key(&root), app));
        native.focus();
    }
}

pub(crate) struct ReviewSession {
    client: Client,
    root: PathBuf,
    head: String,
}

impl ReviewSession {
    pub(crate) fn close(&self) {
        self.client.reviews.borrow_mut().remove(&self.root);
    }

    pub(crate) fn show_start(&self) {
        self.client.show_start();
    }

    pub(crate) fn set_title(&mut self, head: Option<&str>, tx: &mut Tx) {
        if let Some(head) = head {
            if self.head == head {
                return;
            }
            head.clone_into(&mut self.head);
        }
        tx.set_title(format!("{} — {}", repo_name(&self.root), self.head));
    }
}

pub(crate) fn repo_name(root: &Path) -> String {
    root.file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn path_key(path: &Path) -> String {
    path.as_os_str()
        .as_encoded_bytes()
        .escape_ascii()
        .to_string()
}

pub(crate) fn run(options: Options) -> Result<(), String> {
    let Options {
        launch,
        demo_pulse,
        tooling,
    } = options;
    let headless = rune::tooling::Args::parse_from(tooling.clone()).headless();
    if headless || launch == Launch::Demo {
        let spec = match launch {
            Launch::Start => {
                let recents = recents::Recents::new(Prefs::open(APP_ID)).paths().to_vec();
                WindowSpec::from_app(
                    "start",
                    start::app((510.0, 410.0)).init(move || Start::preview(recents)),
                )
                .boxed()
            }
            Launch::Review(repository) => WindowSpec::from_app(
                "review",
                review_app(
                    (1240.0, 820.0),
                    Some(repository),
                    None,
                    headless,
                    demo_pulse,
                ),
            )
            .boxed(),
            Launch::Demo => WindowSpec::from_app(
                "review",
                review_app((1240.0, 820.0), None, None, headless, demo_pulse),
            )
            .boxed(),
        };
        return rune::try_run_windows_with_args(RunOptions::default(), tooling, move |_| [spec]);
    }

    let launch = match launch {
        Launch::Review(mut repository) => {
            // An IPC receiver can have a different working directory.
            repository.path =
                std::path::absolute(repository.path).map_err(|error| error.to_string())?;
            Launch::Review(repository)
        }
        launch => launch,
    };
    run_desktop(launch, tooling)
}

fn run_desktop(launch: Launch, tooling: Vec<String>) -> Result<(), String> {
    if !rune::single_instance(APP_ID) {
        return Ok(());
    }
    let (send, receive) = mpsc::channel();
    #[cfg(target_os = "macos")]
    {
        let send = send.clone();
        if !crate::instance::claim(&launch, move |launch| send.send(launch).is_ok())
            .map_err(|error| format!("Git Viewer launch handoff: {error}"))?
        {
            return Ok(());
        }
    }
    let opens_start = launch == Launch::Start;
    let app = start::app(frame::size("start", (510.0, 410.0)))
        .on_window(|window| frame::restore(window, "start"))
        .keep_running();
    let handle = app.handle();
    let start = handle.clone();
    let client = Client {
        start,
        send,
        reviews: Reviews::default(),
    };
    let app = app.init(move || Start::live(client));
    let app = if opens_start { app } else { app.windowless() };
    thread::Builder::new()
        .name("review-repositories".into())
        .spawn(move || route(launch, &receive, &handle))
        .map_err(|error| format!("Couldn’t start the repository worker: {error}"))?;
    rune::try_run_windows_with_args(RunOptions::default(), tooling, move |_| {
        [WindowSpec::from_app("start", app)]
    })
}

fn review_app(
    size: (f32, f32),
    repository: Option<Repository>,
    session: Option<ReviewSession>,
    headless: bool,
    demo_pulse: bool,
) -> App<State> {
    let app = App::new(AppSpec {
        on_key: Some(State::key),
        scenes: &[("reduced-motion", State::force_reduced_motion)],
        setup: Some(State::setup),
        poll: Some(State::poll),
        families: ("Inter", "ui-monospace"),
        min_size: Some((920.0, 480.0)),
        ..AppSpec::new(
            APP_NAME,
            size,
            || State::open(None, false, false),
            view::review,
        )
    })
    .titlebar(Titlebar::new(view::TOOLBAR_HEIGHT))
    .init(move || {
        let mut state = State::open(repository, headless, demo_pulse);
        state.session = session;
        state
    })
    .on_visibility(State::visibility);
    let handle = app.handle();
    app.on_ready(move |_| {
        handle.update_if(|state, tx| {
            state.title(tx);
            state.request_refresh(tx);
            false
        });
    })
}

fn route(initial: Launch, requests: &Receiver<Launch>, handle: &AppHandle<Start>) {
    let mut recent = recents::Recents::new(Prefs::open(APP_ID));
    let paths = recent.paths().to_vec();
    handle.update(move |state, _| state.set_recents(paths));
    let mut log = std::env::var_os("GIT_VIEWER_LAUNCH_LOG").and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    for (index, request) in std::iter::once(initial).chain(requests.iter()).enumerate() {
        if let Some(log) = &mut log {
            let _ = writeln!(log, "pid={} request={request:?}", std::process::id());
        }
        match request {
            Launch::Review(repository) => {
                let result = discover(repository).map_err(|error| error.to_string());
                let warning = result.as_ref().ok().and_then(|(repository, _)| {
                    recent
                        .visit(&repository.path)
                        .err()
                        .map(|error| error.to_string())
                });
                let paths = recent.paths().to_vec();
                handle.update(move |state, tx| {
                    state.set_recents(paths);
                    state.resolved(result, warning, tx);
                });
            }
            Launch::Start if index > 0 => handle.update(|state, _| state.show()),
            _ => {}
        }
    }
}

fn discover(mut repository: Repository) -> Result<(Repository, String), repo::Error> {
    let repo = repo::Repo::open(&repository.path)?;
    repository.path = repo
        .root()
        .canonicalize()
        .unwrap_or_else(|_| repo.root().to_owned());
    let head = repo.head_name()?.to_string_lossy().into_owned();
    Ok((repository, head))
}

pub(crate) fn open_shortcut(event: &rune::KeyEvent) -> bool {
    event.mods.cmd
        && !event.mods.ctrl
        && !event.mods.alt
        && !event.mods.shift
        && matches!(&event.key, rune::Key::Text(key) if key.eq_ignore_ascii_case("o"))
}
