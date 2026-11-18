//! Run one native window and hand terminal launches to the repository worker.

use std::{path::Path, rc::Rc, sync::mpsc, thread};

use rune::{App, AppSpec, RunOptions, Titlebar, WindowSpec, prefs::Prefs};

use crate::{
    APP_ID, APP_NAME,
    cli::{Launch, Options},
    frame::{self, FRAME_NAME},
    panels::{MIN_WINDOW_WIDTH, Widths},
    preferences::migration,
    repositories::{Request, Workspace},
    state::State,
    view,
};

const SIZE: (f32, f32) = (1360.0, 820.0);

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
        preload,
        demo_pulse,
        tooling,
    } = options;
    let headless = rune::tooling::Args::parse_from(tooling.clone()).headless();
    if headless || launch == Launch::Demo {
        let state = if launch == Launch::Demo {
            State::open(None, headless, demo_pulse)
        } else {
            State::preview_workspace(preload, launch, demo_pulse)
        };
        // Headless runs may opt into a scratch store. Read and write it outside
        // the runner, before any frames are built and after the last frame.
        let mut prefs = (headless && frame::isolated()).then(|| Prefs::open(APP_ID));
        let widths = Rc::clone(&state.panels.preferred);
        let initial = prefs.as_ref().map_or_else(Widths::default, Widths::load);
        widths.set(initial);
        let app = window(SIZE).init(move || state);
        rune::try_run_windows_with_args(RunOptions::default(), tooling, move |_| {
            [WindowSpec::from_app(FRAME_NAME, app)]
        })?;
        if let Some(prefs) = &mut prefs
            && widths.get() != initial
        {
            widths
                .get()
                .save(prefs)
                .map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    if !preload.is_empty() {
        return Err("Repeated --repo flags are for headless captures only.".into());
    }
    let launch = match launch {
        Launch::Review(mut repository) => {
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
        if !crate::instance::claim(&launch, move |launch| {
            send.send(Request::Launch(launch)).is_ok()
        })
        .map_err(|error| format!("{APP_NAME} launch handoff: {error}"))?
        {
            return Ok(());
        }
    }
    // Migration must finish before AppKit reads the frame and the worker loads
    // repository settings. The window and its event loop do not exist yet.
    let prefs = thread::Builder::new()
        .name("migrate-preferences".into())
        .spawn(migration::run)
        .map_err(|error| format!("Couldn’t start preferences migration: {error}"))?
        .join()
        .map_err(|_| "Preferences migration thread failed".to_owned())??;
    let app = window(frame::size(FRAME_NAME, SIZE))
        .on_window(|window| frame::restore(window, FRAME_NAME))
        .keep_running()
        .init(move || State::with_workspace(Workspace::live(send)));
    let handle = app.handle();
    thread::Builder::new()
        .name("review-repositories".into())
        .spawn(move || crate::repositories::route(launch, &receive, &handle, prefs))
        .map_err(|error| format!("Couldn’t start the repository worker: {error}"))?;
    rune::try_run_windows_with_args(RunOptions::default(), tooling, move |_| {
        [WindowSpec::from_app(FRAME_NAME, app)]
    })
}

fn window(size: (f32, f32)) -> App<State> {
    App::new(AppSpec {
        on_key: Some(State::key),
        scenes: &[("reduced-motion", State::force_reduced_motion)],
        setup: Some(State::setup),
        poll: Some(State::poll),
        families: ("Inter", "ui-monospace"),
        min_size: Some((MIN_WINDOW_WIDTH, 480.0)),
        ..AppSpec::new(
            APP_NAME,
            size,
            || State::open(None, false, false),
            view::review,
        )
    })
    .titlebar(Titlebar::new(view::TOOLBAR_HEIGHT))
    .tab_visits_controls(true)
    .on_visibility(State::visibility)
}
