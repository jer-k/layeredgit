//! Schedules Git checks without waiting for them in Rune's callbacks.

use std::sync::Arc;

use repo::{ChangeToken, Error, Repo};
use rune::{Latest, Tx, Waker};

use super::Repository;
use crate::model::Review;

const INTERVAL: f64 = 1.0;

pub(crate) struct Refresh {
    target: Repository,
    pending: Latest<Result<Check, Error>>,
    probe: Option<Probe>,
    visible: bool,
    started: bool,
    forced: bool,
    due: f64,
}

type Probe = (Arc<Repo>, ChangeToken);

struct Check {
    probe: Probe,
    review: Option<Review>,
}

impl Refresh {
    pub(crate) fn new(target: Repository) -> Self {
        Self {
            target,
            pending: Latest::default(),
            probe: None,
            visible: true,
            started: false,
            forced: false,
            due: 0.0,
        }
    }

    pub(crate) fn setup(&self, waker: Waker) {
        self.pending.set_waker(waker);
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

    pub(crate) fn poll(&mut self, now: f64, tx: &mut Tx) -> Option<Result<Review, Error>> {
        // Check before taking the result so a worker finishing here can't have its
        // answer overwritten by the next request.
        let pending = self.pending.is_pending();
        let result = self.pending.take();
        let idle = !pending || result.is_some();
        let landed = result.and_then(|result| match result {
            Ok(Check { probe, review }) => {
                self.probe = Some(probe);
                review.map(Ok)
            }
            Err(error) => {
                // Re-discover the repo on retry, including after an invalid path or missing Git.
                self.probe = None;
                Some(Err(error))
            }
        });
        if self.started && idle {
            if self.forced || (self.visible && now >= self.due) {
                self.due = now + INTERVAL;
                let input = (
                    self.target.clone(),
                    self.probe.clone(),
                    std::mem::take(&mut self.forced),
                );
                self.pending.request(input, |(target, probe, force)| {
                    Self::check(&target, probe, force)
                });
            } else if self.visible {
                tx.wake_at(self.due);
            }
        }
        landed
    }

    fn check(target: &Repository, previous: Option<Probe>, force: bool) -> Result<Check, Error> {
        // The probe only reads status. A repo with an active blob subprocess lives entirely
        // inside load_input, so even dropping a probe on the UI thread cannot wait for Git.
        let repo = match &previous {
            Some((repo, _)) if !force => Arc::clone(repo),
            _ => Arc::new(Repo::open(&target.path)?),
        };
        let token = repo.change_token()?;
        let review = if !force && previous.is_some_and(|(_, previous)| previous == token) {
            None
        } else {
            Some(Review::new(target.load_input()?))
        };
        Ok(Check {
            probe: (repo, token),
            review,
        })
    }
}
