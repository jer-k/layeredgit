//! Schedules Git checks without waiting for them in Rune's callbacks.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::Arc,
};

use repo::{ChangeToken, Error, Repo};
use rune::{Latest, Tx, Waker};

use super::{Comparison, Repository, watch::Watch};
use crate::model::{Review, fresh::Replacement};

const INTERVAL: f64 = 5.0;

pub(crate) struct Refresh {
    target: Repository,
    head: Option<OsString>,
    pending: Latest<Result<Check, Error>>,
    probe: Option<Arc<Probe>>,
    checkout_dirty: Option<bool>,
    watch: Option<(PathBuf, Watch)>,
    waker: Option<Waker>,
    visible: bool,
    forced: bool,
    due: Option<f64>,
    suppress_pulse: bool,
}

struct Probe {
    repo: Arc<Repo>,
    worktree: Option<Arc<Repo>>,
    token: Option<ChangeToken>,
    comparison: Comparison,
}

struct Check {
    probe: Probe,
    watch: Option<(PathBuf, Watch)>,
    event: Option<RefreshEvent>,
}

pub(crate) enum RefreshEvent {
    Updating,
    Reloaded {
        replacement: Replacement,
        worktree: bool,
    },
}

#[derive(Clone, Copy)]
enum Scan {
    Force,
    Poll,
    Returning,
}

impl Refresh {
    pub(crate) fn new(target: Repository) -> Self {
        Self {
            target,
            head: None,
            pending: Latest::default(),
            probe: None,
            checkout_dirty: None,
            watch: None,
            waker: None,
            visible: true,
            forced: false,
            due: None,
            suppress_pulse: true,
        }
    }

    pub(crate) fn setup(&mut self, waker: Waker) {
        self.pending.set_waker(Arc::clone(&waker));
        self.waker = Some(waker);
    }

    pub(crate) fn force(&mut self, tx: &mut Tx) {
        self.due = Some(tx.clock());
        self.forced = true;
        tx.wake_at(tx.clock());
    }

    pub(crate) fn retarget(&mut self, target: Repository, head: Option<OsString>, tx: &mut Tx) {
        self.pending.cancel();
        self.watch = None;
        self.target = target;
        self.head = head;
        self.suppress_pulse = true;
        self.force(tx);
    }

    pub(crate) fn activate(&mut self, tx: &mut Tx) {
        self.suppress_pulse = true;
        self.due = Some(tx.clock());
        tx.wake_at(tx.clock());
    }

    pub(crate) fn suspend(&mut self) {
        self.pending.cancel();
        self.watch = None;
        self.due = None;
    }

    pub(crate) fn take_checkout_dirty(&mut self) -> Option<bool> {
        self.checkout_dirty.take()
    }

    pub(crate) fn visibility(&mut self, visible: bool, tx: &mut Tx) {
        self.visible = visible;
        if !visible {
            self.pending.cancel();
            self.watch = None;
        }
        if visible && self.due.is_some() {
            self.due = Some(tx.clock());
            tx.wake_at(tx.clock());
        }
    }

    pub(crate) fn poll(
        &mut self,
        now: f64,
        current: &Arc<Review>,
        tx: &mut Tx,
    ) -> Option<Result<RefreshEvent, Error>> {
        // Check before taking the result so a worker finishing here can't have its
        // answer overwritten by the next request.
        let pending = self.pending.is_pending();
        let result = self.pending.take();
        let idle = !pending || result.is_some();
        let landed = result.and_then(|result| match result {
            Ok(Check {
                probe,
                watch,
                mut event,
            }) => {
                if self.watch.as_ref().is_some_and(|(root, _)| {
                    Some(root.as_path()) != probe.comparison.revisions.worktree()
                }) {
                    self.watch = None;
                }
                if watch.is_some() {
                    self.watch = watch;
                }
                if probe.comparison.revisions.worktree() == Some(probe.repo.root()) {
                    self.checkout_dirty = probe.token.map(ChangeToken::is_dirty);
                }
                self.probe = Some(Arc::new(probe));
                match &mut event {
                    Some(RefreshEvent::Updating) => self.forced = true,
                    Some(RefreshEvent::Reloaded { replacement, .. }) => {
                        if std::mem::take(&mut self.suppress_pulse) {
                            replacement.fresh.clear();
                        }
                    }
                    None => self.suppress_pulse = false,
                }
                event.map(Ok)
            }
            Err(error) => {
                // Re-discover the repo on retry, including after an invalid path or missing Git.
                self.probe = None;
                self.watch = None;
                Some(Err(error))
            }
        });
        if let Some(due) = self.due
            && idle
        {
            let changed = self.visible
                && self
                    .watch
                    .as_ref()
                    .is_some_and(|(_, watch)| watch.take_changed());
            if self.visible && (self.forced || changed || now >= due) {
                self.due = Some(now + INTERVAL);
                let mode = if std::mem::take(&mut self.forced) || changed {
                    Scan::Force
                } else if self.suppress_pulse && self.probe.is_some() {
                    Scan::Returning
                } else {
                    Scan::Poll
                };
                let current = match &landed {
                    Some(Ok(RefreshEvent::Reloaded { replacement, .. })) => &replacement.review,
                    _ => current,
                };
                let input = (
                    self.target.clone(),
                    self.head.clone(),
                    self.probe.clone(),
                    self.waker.clone(),
                    self.watch.as_ref().map(|(path, _)| path.clone()),
                    mode,
                    Arc::clone(current),
                );
                self.pending.request(
                    input,
                    |(target, head, probe, waker, watching, mode, current)| {
                        Self::check(
                            &target,
                            head.as_deref(),
                            probe.as_deref(),
                            waker,
                            watching.as_deref(),
                            mode,
                            &current,
                        )
                    },
                );
            } else if self.visible {
                tx.wake_at(due);
            }
        }
        landed
    }

    fn check(
        target: &Repository,
        head: Option<&OsStr>,
        previous: Option<&Probe>,
        waker: Option<Waker>,
        watching: Option<&Path>,
        mode: Scan,
        current: &Review,
    ) -> Result<Check, Error> {
        // Only the repo passed to read_input opens a blob process.
        let repo = match previous {
            Some(previous) => Arc::clone(&previous.repo),
            _ => Arc::new(Repo::open(&target.path)?),
        };
        let comparison = target.comparison(&repo, head)?;
        let worktree = match comparison.revisions.worktree() {
            Some(root) if root == repo.root() => Some(Arc::clone(&repo)),
            Some(root) => Some(match previous.and_then(|probe| probe.worktree.as_ref()) {
                Some(old) if old.root() == root => Arc::clone(old),
                _ => Arc::new(Repo::open(root)?),
            }),
            None => None,
        };
        let token = worktree
            .as_ref()
            .map(|repo| repo.change_token())
            .transpose()?;
        let unchanged =
            previous.is_some_and(|old| old.token == token && old.comparison == comparison);
        let event = match (mode, unchanged) {
            (Scan::Poll | Scan::Returning, true) => None,
            (Scan::Returning, false) => Some(RefreshEvent::Updating),
            (Scan::Force, _) | (Scan::Poll, false) => {
                let review = Review::new(Repository::read_input(
                    Repo::open(repo.root())?,
                    &comparison,
                )?);
                let mut replacement = Replacement::new(review, current);
                if previous.is_some_and(|old| {
                    old.comparison.revisions.worktree() != comparison.revisions.worktree()
                }) {
                    replacement.fresh.clear();
                }
                Some(RefreshEvent::Reloaded {
                    replacement,
                    worktree: worktree.is_some(),
                })
            }
        };
        let watch = worktree
            .as_ref()
            .filter(|repo| watching != Some(repo.root()))
            .and_then(|repo| {
                waker
                    .and_then(|waker| Watch::start(Arc::clone(repo), waker))
                    .map(|watch| (repo.root().to_owned(), watch))
            });
        Ok(Check {
            probe: Probe {
                repo,
                worktree,
                token,
                comparison,
            },
            watch,
            event,
        })
    }
}
