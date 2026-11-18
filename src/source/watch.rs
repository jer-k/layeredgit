//! Debounces filesystem notifications and checks ignore rules off the UI thread.

mod ignore;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use repo::Repo;
use rune::Waker;

use self::ignore::Ignore;

const DEBOUNCE: Duration = Duration::from_millis(200);

enum Message {
    Event(notify::Result<Event>),
    Stop,
}

pub(super) struct Watch {
    send: Sender<Message>,
    changed: Arc<AtomicBool>,
}

impl Watch {
    pub(super) fn start(repo: Arc<Repo>, waker: Waker) -> Option<Self> {
        let (send, receive) = mpsc::channel();
        let events = send.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = events.send(Message::Event(event));
        })
        .ok()?;
        subscribe(&mut watcher, &repo).ok()?;
        let changed = Arc::new(AtomicBool::new(false));
        let dirty = Arc::clone(&changed);
        thread::Builder::new()
            .name("review-watch".into())
            .spawn(move || {
                listen(&repo, &receive, &dirty, &waker);
                drop(watcher);
            })
            .ok()?;
        Some(Self { send, changed })
    }

    pub(super) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.send.send(Message::Stop);
    }
}

fn subscribe(watcher: &mut RecommendedWatcher, repo: &Repo) -> notify::Result<()> {
    watcher.watch(repo.root(), RecursiveMode::Recursive)?;
    if !repo.common_dir().starts_with(repo.root()) {
        watcher.watch(repo.common_dir(), RecursiveMode::Recursive)?;
    }
    if !repo.git_dir().starts_with(repo.root()) && !repo.git_dir().starts_with(repo.common_dir()) {
        watcher.watch(repo.git_dir(), RecursiveMode::Recursive)?;
    }
    Ok(())
}

fn listen(repo: &Repo, receive: &Receiver<Message>, dirty: &AtomicBool, waker: &Waker) {
    let mut ignore = Ignore::default();
    while let Ok(Message::Event(first)) = receive.recv() {
        let mut batch = Batch::default();
        batch.collect(repo, first);
        let deadline = Instant::now() + DEBOUNCE;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match receive.recv_timeout(remaining) {
                Ok(Message::Event(event)) => batch.collect(repo, event),
                Err(RecvTimeoutError::Timeout) => break,
                Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if batch.affects_review(repo, &mut ignore) && !dirty.swap(true, Ordering::AcqRel) {
            waker();
        }
    }
}

#[derive(Default)]
struct Batch {
    reload: bool,
    reset_ignores: bool,
    read_config: bool,
    worktree: HashSet<PathBuf>,
}

impl Batch {
    fn collect(&mut self, repo: &Repo, event: notify::Result<Event>) {
        let Ok(event) = event else {
            // The event stream lost information. Read a fresh snapshot.
            self.reload = true;
            self.reset_ignores = true;
            self.read_config = true;
            return;
        };
        if event.need_rescan() {
            self.reload = true;
            self.reset_ignores = true;
            self.read_config = true;
        } else if !matches!(event.kind, EventKind::Access(_)) {
            for path in &event.paths {
                self.collect_path(repo, path);
            }
        }
    }

    fn collect_path(&mut self, repo: &Repo, path: &Path) {
        if let Ok(path) = path.strip_prefix(repo.git_dir()) {
            self.reload |= local_metadata(path);
            self.reset_ignores |= path == Path::new("index");
            self.read_config |= path == Path::new("config.worktree");
        }
        if let Ok(path) = path.strip_prefix(repo.common_dir()) {
            self.reload |= shared_metadata(path);
            self.reset_ignores |= path == Path::new("info/exclude");
            self.read_config |= path == Path::new("config");
        } else if let Ok(path) = path.strip_prefix(repo.root())
            && !path.starts_with(".git")
            && !path.as_os_str().is_empty()
        {
            if path.file_name().is_some_and(|name| name == ".gitignore") {
                self.reset_ignores = true;
                self.reload = true;
            }
            self.worktree.insert(path.to_owned());
        }
    }

    fn affects_review(self, repo: &Repo, ignore: &mut Ignore) -> bool {
        if !self.reload && self.worktree.is_empty() {
            return false;
        }
        let rules_changed = ignore.update_rules(repo, self.reset_ignores, self.read_config);
        if self.reload || rules_changed {
            return true;
        }
        ignore.affects_review(repo, self.worktree)
    }
}

fn local_metadata(path: &Path) -> bool {
    ["HEAD", "index", "config.worktree"]
        .iter()
        .any(|name| path == Path::new(name))
}

fn shared_metadata(path: &Path) -> bool {
    (path.starts_with("refs") && path.extension().is_none_or(|ext| ext != "lock"))
        || [
            "packed-refs",
            "config",
            "info/exclude",
            "shallow",
            "reftable",
        ]
        .iter()
        .any(|name| path == Path::new(name))
}
