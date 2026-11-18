//! Schedules Git checks without waiting for them in Rune's callbacks.

use std::sync::Arc;

use repo::{ChangeToken, Error, Repo};
use rune::{Latest, Tx, Waker};

use super::{Comparison, Repository, watch::Watch};
use crate::model::{Review, fresh::Replacement};

const INTERVAL: f64 = 5.0;

pub(crate) struct Refresh {
    target: Repository,
    pending: Latest<Result<Check, Error>>,
    probe: Option<Arc<Probe>>,
    waker: Option<Waker>,
    visible: bool,
    started: bool,
    forced: bool,
    due: f64,
}

struct Probe {
    repo: Arc<Repo>,
    watch: Option<Arc<Watch>>,
    token: ChangeToken,
    comparison: Comparison,
}

struct Check {
    probe: Probe,
    review: Option<Replacement>,
}

impl Refresh {
    pub(crate) fn new(target: Repository) -> Self {
        Self {
            target,
            pending: Latest::default(),
            probe: None,
            waker: None,
            visible: true,
            started: false,
            forced: false,
            due: 0.0,
        }
    }

    pub(crate) fn setup(&mut self, waker: Waker) {
        self.pending.set_waker(Arc::clone(&waker));
        self.waker = Some(waker);
    }

    pub(crate) fn force(&mut self, tx: &mut Tx) {
        self.started = true;
        self.forced = true;
        tx.wake_at(tx.clock());
    }

    pub(crate) fn visibility(&mut self, visible: bool, tx: &mut Tx) {
        self.visible = visible;
        if visible && self.started {
            self.due = tx.clock();
            tx.wake_at(self.due);
        }
    }

    pub(crate) fn poll(
        &mut self,
        now: f64,
        current: &Arc<Review>,
        tx: &mut Tx,
    ) -> Option<Result<Replacement, Error>> {
        // Check before taking the result so a worker finishing here can't have its
        // answer overwritten by the next request.
        let pending = self.pending.is_pending();
        let result = self.pending.take();
        let idle = !pending || result.is_some();
        let landed = result.and_then(|result| match result {
            Ok(Check { probe, review }) => {
                self.probe = Some(Arc::new(probe));
                review.map(Ok)
            }
            Err(error) => {
                // Re-discover the repo on retry, including after an invalid path or missing Git.
                self.probe = None;
                Some(Err(error))
            }
        });
        if self.started && idle {
            let changed = self.visible
                && self
                    .probe
                    .as_ref()
                    .and_then(|probe| probe.watch.as_deref())
                    .is_some_and(Watch::take_changed);
            if self.forced || changed || (self.visible && now >= self.due) {
                self.due = now + INTERVAL;
                let force = std::mem::take(&mut self.forced) || changed;
                let current = landed
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .map_or(current, |update| &update.review);
                let input = (
                    self.target.clone(),
                    self.probe.clone(),
                    self.waker.clone(),
                    force,
                    Arc::clone(current),
                );
                self.pending
                    .request(input, |(target, probe, waker, force, current)| {
                        Self::check(&target, probe.as_deref(), waker, force, &current)
                    });
            } else if self.visible {
                tx.wake_at(self.due);
            }
        }
        landed
    }

    fn check(
        target: &Repository,
        previous: Option<&Probe>,
        waker: Option<Waker>,
        force: bool,
        current: &Review,
    ) -> Result<Check, Error> {
        // Only the repo passed to read_input opens a blob process.
        let repo = match previous {
            Some(previous) => Arc::clone(&previous.repo),
            _ => Arc::new(Repo::open(&target.path)?),
        };
        let watch = previous.and_then(|probe| probe.watch.clone()).or_else(|| {
            waker
                .and_then(|waker| Watch::start(Arc::clone(&repo), waker))
                .map(Arc::new)
        });
        let token = repo.change_token()?;
        let comparison = target.comparison(&repo)?;
        let review = if !force
            && previous.is_some_and(|old| old.token == token && old.comparison == comparison)
        {
            None
        } else {
            let review = Review::new(target.read_input(Repo::open(repo.root())?, &comparison)?);
            Some(Replacement::new(review, current))
        };
        Ok(Check {
            probe: Probe {
                repo,
                watch,
                token,
                comparison,
            },
            review,
        })
    }
}
