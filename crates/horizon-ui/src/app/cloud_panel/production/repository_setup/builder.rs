//! The rented builder that setups chosen to **Build on: Rented builder** share: Horizon
//! rents it when the first such setup starts, its proofs wait until it answers, and it is
//! deleted once no setup has used it for [`IDLE_RETURN`], and as Horizon exits. A builder
//! that an earlier Horizon left, after a crash for instance, is deleted at the next start,
//! and a deletion that failed is tried again until it succeeds. Only the Horizon that
//! claimed the builder of the cloud root rents or deletes it, and it lets the claim go
//! while it has no builder. All provider and file work runs on threads; the frame only
//! reads their outcome.
use horizon_core::cloud_runtime::{
    Cancellation,
    docker_host::Host,
    settings::Settings,
    setup_builder::{self, Owner},
};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// How long a builder no setup uses stays, so the next setup does not wait for a new one.
const IDLE_RETURN: Duration = Duration::from_mins(10);
/// How long a failed renting or deletion waits before it is tried again.
const RETRY_AFTER: Duration = Duration::from_mins(2);
/// A renting that failed this often is not tried again until another setup starts wanting
/// a builder: each try can bill a server.
const MAX_RENTINGS: u32 = 3;
/// How long Horizon's exit waits for the builder's deletion; the journal keeps the rest
/// for the next start.
const EXIT_WAIT: Duration = Duration::from_secs(60);

enum Status {
    /// The claim on the cloud root's builder is not taken yet.
    Claiming,
    /// Another process on this computer held the claim; it is tried again later.
    Elsewhere {
        reason: String,
        at: Instant,
    },
    Idle,
    Renting(setup_builder::Stage),
    Ready {
        builder: setup_builder::Builder,
        since: Instant,
    },
    Returning,
    Failed {
        reason: String,
        at: Instant,
        /// A builder may still exist: the next try deletes it.
        left: bool,
    },
}

enum Outcome {
    Claimed(Result<Option<Owner>, String>, Result<(), String>),
    Stage(setup_builder::Stage),
    Rented(Result<setup_builder::Builder, (String, bool)>),
    Returned(Result<(), String>),
}

/// The rented builder of this Horizon.
pub(in crate::app::cloud_panel::production) struct Builder {
    status: Status,
    owner: Option<Arc<Owner>>,
    job: Option<mpsc::Receiver<Outcome>>,
    cancel: Cancellation,
    /// No setup has wanted the builder since then.
    unused_since: Option<Instant>,
    /// Rentings that failed since a setup last started wanting a builder.
    failures: u32,
    /// The setups that want the builder, by their run's agent panel.
    demand: BTreeSet<u64>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            status: Status::Claiming,
            owner: None,
            job: None,
            cancel: Cancellation::default(),
            unused_since: None,
            failures: 0,
            demand: BTreeSet::new(),
        }
    }
}

impl Builder {
    /// Takes what the threads found, claims the builder and deletes one left from before,
    /// rents one while the setups of `wanting` want it and none is there, deletes one no
    /// setup has wanted for a while, and tries a failed renting or deletion again.
    pub(in crate::app::cloud_panel::production) fn tick(
        &mut self,
        wanting: &BTreeSet<u64>,
        root: Option<&Path>,
        ctx: &egui::Context,
    ) {
        self.poll();
        let Some(root) = root else {
            return;
        };
        let wanted = !wanting.is_empty();
        // A setup that starts wanting a builder may try renting again, also while others
        // still wait for one.
        if wanting.iter().any(|run| !self.demand.contains(run)) {
            self.failures = 0;
        }
        self.demand.clone_from(wanting);
        self.unused_since = if wanted {
            None
        } else {
            self.unused_since.or_else(|| Some(Instant::now()))
        };
        if self.job.is_some() {
            return;
        }
        let retry = |at: &Instant| at.elapsed() >= RETRY_AFTER;
        let may_rent = self.failures < MAX_RENTINGS;
        match &self.status {
            Status::Claiming => self.claim(root, ctx),
            Status::Elsewhere { at, .. } if wanted && retry(at) => self.claim(root, ctx),
            Status::Idle if wanted && self.owner.is_none() => self.claim(root, ctx),
            Status::Idle if wanted && may_rent => self.rent(ctx),
            Status::Failed { at, left: true, .. } if retry(at) => self.give_back(ctx),
            Status::Failed { at, left: false, .. } if wanted && may_rent && retry(at) => match self.owner {
                Some(_) => self.rent(ctx),
                None => self.claim(root, ctx),
            },
            // Without a builder the claim goes, so another Horizon can rent one, and a
            // later setup claims it again before it rents.
            Status::Idle if !wanted => self.owner = None,
            Status::Failed { left: false, .. } if !wanted => {
                self.owner = None;
                self.status = Status::Idle;
            }
            Status::Ready { .. } if self.unused_since.is_some_and(|since| since.elapsed() >= IDLE_RETURN) => {
                self.give_back(ctx);
            }
            _ => {}
        }
        if wanted || matches!(self.status, Status::Ready { .. } | Status::Failed { left: true, .. }) {
            ctx.request_repaint_after(Duration::from_secs(5));
        }
    }

    /// The builder as a Docker host, once it answers.
    pub(in crate::app::cloud_panel::production) fn host(&self) -> Option<Host> {
        match &self.status {
            Status::Ready { builder, .. } => Some(builder.host.clone()),
            _ => None,
        }
    }

    /// Why the builder is not there, for a setup that waits for it, and whether that is a
    /// failure to show.
    pub(in crate::app::cloud_panel::production) fn waiting(&self) -> Option<(String, bool)> {
        let stage = |stage| match stage {
            setup_builder::Stage::Choosing => "Renting a builder: choosing a server",
            setup_builder::Stage::Creating => "Renting a builder: creating the server",
            setup_builder::Stage::Starting => "Renting a builder: waiting for its Docker",
        };
        match &self.status {
            Status::Ready { .. } => None,
            Status::Claiming | Status::Idle => Some(("Renting a builder".to_owned(), false)),
            Status::Renting(now) => Some((stage(*now).to_owned(), false)),
            Status::Returning => Some(("Waiting while the last builder is deleted".to_owned(), false)),
            Status::Elsewhere { reason, .. } => Some((reason.clone(), true)),
            Status::Failed { reason, left: true, .. } => Some((
                format!("The rented builder is not deleted yet, and Horizon tries again: {reason}"),
                true,
            )),
            Status::Failed { reason, .. } if self.failures >= MAX_RENTINGS => Some((
                format!("The builder could not be rented; start the setup again to try once more: {reason}"),
                true,
            )),
            Status::Failed { reason, .. } => Some((
                format!("The builder could not be rented, and Horizon tries again: {reason}"),
                true,
            )),
        }
    }

    /// One line about the builder for the setup board: what runs and what it costs, or
    /// what happens to it. None while there is no builder and nothing failed.
    pub(in crate::app::cloud_panel::production) fn line(&self) -> Option<(String, bool)> {
        match &self.status {
            Status::Claiming | Status::Idle | Status::Elsewhere { .. } => None,
            Status::Ready { builder, since } => {
                let minutes = since.elapsed().as_secs() / 60;
                let idle = self.unused_since.map_or(String::new(), |unused| {
                    let left = IDLE_RETURN.saturating_sub(unused.elapsed()).as_secs().div_ceil(60);
                    format!(", deleted in {left} min unless a setup uses it")
                });
                Some((
                    format!(
                        "Rented builder {} in {}: €{:.3} an hour, running {minutes} min{idle}",
                        builder.server_type, builder.location, builder.hourly_eur
                    ),
                    false,
                ))
            }
            Status::Returning => Some(("Deleting the rented builder".to_owned(), false)),
            _ => self.waiting(),
        }
    }

    /// Stops a renting and deletes the builder as Horizon exits, waiting up to
    /// [`EXIT_WAIT`] in all. A builder still there after that is deleted at the next start.
    pub(in crate::app::cloud_panel::production) fn give_back_for_exit(&mut self) {
        self.cancel.cancel();
        let deadline = Instant::now() + EXIT_WAIT;
        // A renting thread deletes what it made once it sees the cancellation, and a
        // claiming one may hand over the claim.
        if let Some(job) = self.job.take() {
            while let Ok(outcome) = job.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                match outcome {
                    Outcome::Stage(_) => {}
                    Outcome::Claimed(Ok(Some(owner)), _) => {
                        self.owner = Some(Arc::new(owner));
                        break;
                    }
                    _ => break,
                }
            }
        }
        let Some(owner) = self.owner.clone() else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(give_back(&owner, &Cancellation::default()));
        });
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!("the rented builder is not deleted yet; the next start tries again: {error}");
            }
            Err(_) => tracing::warn!("Horizon exits while the rented builder is deleted; the next start tries again"),
        }
    }

    fn poll(&mut self) {
        let Some(job) = self.job.take() else {
            return;
        };
        loop {
            let outcome = match job.try_recv() {
                Ok(outcome) => outcome,
                Err(mpsc::TryRecvError::Empty) => {
                    self.job = Some(job);
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // A thread that ended without an outcome may have left a builder.
                    if !matches!(self.status, Status::Ready { .. } | Status::Idle) {
                        self.failed("the builder's thread ended".to_owned(), true);
                    }
                    return;
                }
            };
            match outcome {
                Outcome::Stage(stage) => {
                    self.status = Status::Renting(stage);
                    continue;
                }
                Outcome::Claimed(Ok(Some(owner)), returned) => {
                    self.owner = Some(Arc::new(owner));
                    match returned {
                        Ok(()) => self.status = Status::Idle,
                        Err(reason) => self.failed(reason, true),
                    }
                }
                Outcome::Claimed(Ok(None), _) => {
                    self.status = Status::Elsewhere {
                        reason: "Another Horizon on this computer has a rented builder; this setup waits until it lets it go"
                            .to_owned(),
                        at: Instant::now(),
                    };
                }
                Outcome::Claimed(Err(reason), _) => {
                    self.status = Status::Elsewhere {
                        reason: format!("The rented builder cannot be used now: {reason}"),
                        at: Instant::now(),
                    };
                }
                Outcome::Rented(Ok(builder)) => {
                    self.failures = 0;
                    self.status = Status::Ready {
                        builder,
                        since: Instant::now(),
                    };
                }
                Outcome::Rented(Err((reason, left))) => {
                    self.failures += 1;
                    self.failed(reason, left);
                }
                Outcome::Returned(Ok(())) => self.status = Status::Idle,
                Outcome::Returned(Err(reason)) => self.failed(reason, true),
            }
            return;
        }
    }

    fn failed(&mut self, reason: String, left: bool) {
        self.status = Status::Failed {
            reason,
            at: Instant::now(),
            left,
        };
    }

    fn claim(&mut self, root: &Path, ctx: &egui::Context) {
        let (sender, receiver) = mpsc::channel();
        let (root, wake) = (root.to_owned(), ctx.clone());
        std::thread::spawn(move || {
            let claimed = setup_builder::claim(&root).map_err(|error| error.to_string());
            let returned = match &claimed {
                Ok(Some(owner)) if setup_builder::left_behind(&root) => give_back(owner, &Cancellation::default()),
                _ => Ok(()),
            };
            let _ = sender.send(Outcome::Claimed(claimed, returned));
            wake.request_repaint();
        });
        self.job = Some(receiver);
    }

    fn rent(&mut self, ctx: &egui::Context) {
        let Some(owner) = self.owner.clone() else {
            return;
        };
        self.status = Status::Renting(setup_builder::Stage::Choosing);
        let (sender, receiver) = mpsc::channel();
        let (wake, cancel) = (ctx.clone(), self.cancel.clone());
        std::thread::spawn(move || {
            let (staged, stage_wake) = (sender.clone(), wake.clone());
            let rented = settings(&owner)
                .and_then(|settings| {
                    setup_builder::rent(&settings, &owner, &cancel, |stage| {
                        let _ = staged.send(Outcome::Stage(stage));
                        stage_wake.request_repaint();
                    })
                    .map_err(|error| error.to_string())
                })
                .map_err(|reason| (reason, setup_builder::left_behind(owner.root())));
            let _ = sender.send(Outcome::Rented(rented));
            wake.request_repaint();
        });
        self.job = Some(receiver);
    }

    fn give_back(&mut self, ctx: &egui::Context) {
        let Some(owner) = self.owner.clone() else {
            return;
        };
        self.status = Status::Returning;
        let (sender, receiver) = mpsc::channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(Outcome::Returned(give_back(&owner, &Cancellation::default())));
            wake.request_repaint();
        });
        self.job = Some(receiver);
    }
}

fn settings(owner: &Owner) -> Result<Settings, String> {
    Settings::load(&owner.root().join("settings.json")).map_err(|error| error.to_string())
}

/// Deletes the builder the owner's journal names. Setup resources on it went with it.
fn give_back(owner: &Owner, cancel: &Cancellation) -> Result<(), String> {
    let settings = settings(owner)?;
    setup_builder::give_back(&settings, owner, cancel).map_err(|error| error.to_string())?;
    super::proof::forget_on(setup_builder::NAME);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setups that want a builder: one when `wanted`, none otherwise.
    fn want(wanted: bool) -> BTreeSet<u64> {
        if wanted { BTreeSet::from([1]) } else { BTreeSet::new() }
    }

    /// Ticks until no thread works, at most ten seconds.
    fn settle(builder: &mut Builder, wanted: bool, root: &Path, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(10);
        builder.tick(&want(wanted), Some(root), ctx);
        while builder.job.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            builder.tick(&want(wanted), Some(root), ctx);
        }
    }

    #[test]
    fn nothing_is_rented_without_a_setup_that_wants_it_or_without_a_cloud_root() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut builder = Builder::default();
        builder.tick(&want(true), None, &ctx);
        assert!(builder.job.is_none(), "no cloud root, no claim");
        settle(&mut builder, false, root.path(), &ctx);
        assert!(matches!(builder.status, Status::Idle));
        assert!(builder.line().is_none());
        assert!(builder.owner.is_none(), "without a builder the claim goes");
        assert!(setup_builder::claim(root.path()).unwrap().is_some());
    }

    #[test]
    fn a_failed_renting_is_told_and_tried_again_only_later() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut builder = Builder::default();
        settle(&mut builder, true, root.path(), &ctx);
        // No settings file: the renting fails at once.
        settle(&mut builder, true, root.path(), &ctx);
        let (text, alert) = builder.waiting().unwrap();
        assert!(alert && text.starts_with("The builder could not be rented"), "{text}");
        builder.tick(&want(true), Some(root.path()), &ctx);
        assert!(builder.job.is_none(), "no second try before {RETRY_AFTER:?}");
        assert!(builder.host().is_none());
    }

    #[test]
    fn a_second_horizon_on_the_same_root_leaves_the_builder_alone() {
        let root = tempfile::tempdir().unwrap();
        let first = setup_builder::claim(root.path()).unwrap().unwrap();
        let ctx = egui::Context::default();
        let mut builder = Builder::default();
        settle(&mut builder, true, root.path(), &ctx);
        assert!(builder.owner.is_none());
        let (text, alert) = builder.waiting().unwrap();
        assert!(alert && text.starts_with("Another Horizon"), "{text}");
        builder.tick(&want(true), Some(root.path()), &ctx);
        assert!(builder.job.is_none(), "it neither rents nor deletes");
        // Once the other lets go, a later try claims.
        drop(first);
        builder.status = Status::Elsewhere {
            reason: String::new(),
            at: Instant::now().checked_sub(RETRY_AFTER).unwrap(),
        };
        settle(&mut builder, true, root.path(), &ctx);
        assert!(builder.owner.is_some() || !matches!(builder.status, Status::Elsewhere { .. }));
    }

    #[test]
    fn renting_stops_after_some_failures_until_a_new_setup_wants_a_builder() {
        let mut builder = Builder {
            failures: MAX_RENTINGS,
            demand: want(true),
            ..Builder::default()
        };
        builder.failed("refused".to_owned(), false);
        let (text, _) = builder.waiting().unwrap();
        assert!(text.contains("start the setup again"), "{text}");
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        builder.status = Status::Failed {
            reason: String::new(),
            at: Instant::now().checked_sub(RETRY_AFTER).unwrap(),
            left: false,
        };
        builder.owner = setup_builder::claim(root.path()).unwrap().map(Arc::new);
        builder.tick(&want(true), Some(root.path()), &ctx);
        assert!(builder.job.is_none(), "no further renting");
        // Another setup starts wanting one while the first still waits.
        builder.tick(&BTreeSet::from([1, 2]), Some(root.path()), &ctx);
        assert_eq!(builder.failures, 0, "a new setup may try again");
        assert!(builder.job.is_some(), "and it rents at once");
        settle(&mut builder, true, root.path(), &ctx);
    }

    #[test]
    fn a_setup_after_a_failed_renting_claims_again_before_it_rents() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut builder = Builder {
            owner: setup_builder::claim(root.path()).unwrap().map(Arc::new),
            ..Builder::default()
        };
        builder.failed("refused".to_owned(), false);
        // The last setup stopped: the claim goes, and the failure with it.
        builder.tick(&want(false), Some(root.path()), &ctx);
        assert!(builder.owner.is_none());
        assert!(matches!(builder.status, Status::Idle));
        // A later setup claims first; without a claim a renting would do nothing.
        settle(&mut builder, true, root.path(), &ctx);
        assert!(builder.owner.is_some(), "the claim is taken again");
    }
}
