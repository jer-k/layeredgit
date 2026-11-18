//! Run one native window and hand terminal launches to the repository worker.

use std::{path::Path, sync::mpsc, thread};

use rune::{App, AppSpec, RunOptions, Titlebar, WindowSpec};

use crate::{
    APP_ID, APP_NAME,
    cli::{Launch, Options},
    frame,
    repositories::{Request, Workspace},
    state::State,
    view,
};

const FRAME: &str = "workspace";
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
        let app = window(SIZE).init(move || state);
        return rune::try_run_windows_with_args(RunOptions::default(), tooling, move |_| {
            [WindowSpec::from_app(FRAME, app)]
        });
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
        .map_err(|error| format!("Git Viewer launch handoff: {error}"))?
        {
            return Ok(());
        }
    }
    let app = window(frame::size(FRAME, SIZE))
        .on_window(|window| frame::restore(window, FRAME))
        .keep_running()
        .init(move || State::with_workspace(Workspace::live(send)));
    let handle = app.handle();
    thread::Builder::new()
        .name("review-repositories".into())
        .spawn(move || crate::repositories::route(launch, &receive, &handle))
        .map_err(|error| format!("Couldn’t start the repository worker: {error}"))?;
    rune::try_run_windows_with_args(RunOptions::default(), tooling, move |_| {
        [WindowSpec::from_app(FRAME, app)]
    })
}

fn window(size: (f32, f32)) -> App<State> {
    App::new(AppSpec {
        on_key: Some(State::key),
        scenes: &[("reduced-motion", State::force_reduced_motion)],
        setup: Some(State::setup),
        poll: Some(State::poll),
        families: ("Inter", "ui-monospace"),
        min_size: Some((960.0, 480.0)),
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
