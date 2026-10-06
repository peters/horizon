//! Idle stop of ready clouds. Where the worker cannot stop itself (see
//! `provider::IdleStop::Horizon`), Horizon reads its worker's idle record every few
//! minutes and stops the cloud after its idle period, as Stop does. Where the worker
//! stops itself (`provider::IdleStop::Worker`), Horizon only reads the record, so that
//! it can name the idle stop when the worker goes away and the provider confirms
//! that it stopped. The watch has its own channel, so a presentation failure on the
//! released server can never hide the stop.
use super::{Deployment, Runtime, Settings, Stage, Store, cloud_runtime};
use cloud_runtime::{
    Cancellation, Error,
    lifecycle::{IdleCheck, IdleSample, StopCause},
    provider::{Description, IdleStop, StoppedCost},
};
use std::{
    path::Path,
    sync::mpsc::{Receiver, Sender, TryRecvError},
    time::{Duration, Instant},
};

/// How often the idle record is read. The worker rewrites it every minute.
const CHECK_INTERVAL: Duration = Duration::from_secs(120);

/// What the idle watch tells the cloud's card.
pub(super) enum Report {
    /// Horizon stopped the cloud, or began to and must be asked to finish; the
    /// saved record, when it could be read back, and the line to log.
    Stopped(Option<Box<Deployment>>, String),
    /// The worker could not be read, and the provider confirmed that it stopped
    /// without Horizon: the saved stopped record.
    StoppedOutside(Box<Deployment>),
    /// The worker's idle record, read now.
    Sampled(IdleSample),
    /// A check failed; logged once until the failure changes.
    Failed(String),
    /// A check is about to run. Sent only to learn whether the card still listens,
    /// so a watch whose card is gone, as after a session switch, ends before it
    /// reads the record or stops anything.
    Checked,
}

pub(super) type Reports = Receiver<Report>;

/// Whether Horizon reads this cloud's idle record: it has an idle period that
/// Horizon or the worker itself applies.
fn watched(state: &Deployment) -> bool {
    state.profile.idle_stop_minutes.is_some()
        && matches!(
            Description::of(&state.profile).idle_stop,
            IdleStop::Horizon | IdleStop::Worker
        )
}

/// Starts the idle watch of a ready cloud with an idle period. It ends with
/// `cancel`, once the cloud is no longer watched, or when its card stops listening.
pub(super) fn watch(
    state: &Deployment,
    settings: &Settings,
    root: &Path,
    cancel: &Cancellation,
    reports: Sender<Report>,
    ctx: &egui::Context,
) {
    if !watched(state) {
        return;
    }
    let (settings, root, cancel, ctx) = (settings.clone(), root.to_owned(), cancel.clone(), ctx.clone());
    std::thread::spawn(move || {
        run(
            &cancel,
            CHECK_INTERVAL,
            |cancel| cloud_runtime::lifecycle::idle_check(&root, &settings, cancel),
            || Store::lock(&root).and_then(|store| store.load()),
            |cancel| cloud_runtime::lifecycle::worker_stopped(&root, &settings, cancel),
            &|report| {
                let repaint = !matches!(report, Report::Checked | Report::Sampled(_));
                let delivered = reports.send(report).is_ok();
                if repaint {
                    ctx.request_repaint();
                }
                delivered
            },
        );
    });
}

/// Checks every `interval` until the cloud stops, is no longer watched, or
/// `report` finds nobody listening. A check that cannot read the worker asks the
/// provider through `confirm` whether the worker stopped without Horizon.
fn run(
    cancel: &Cancellation,
    interval: Duration,
    check: impl Fn(&Cancellation) -> cloud_runtime::Result<IdleCheck>,
    load: impl Fn() -> cloud_runtime::Result<Option<Deployment>>,
    confirm: impl Fn(&Cancellation) -> cloud_runtime::Result<Option<Deployment>>,
    report: &dyn Fn(Report) -> bool,
) {
    let mut failed = None;
    while wait(cancel, interval) {
        if !report(Report::Checked) {
            return;
        }
        let result = check(cancel);
        if cancel.is_cancelled() {
            return;
        }
        match result {
            Ok(IdleCheck::Active { idle, limit }) => {
                failed = None;
                if !report(Report::Sampled(IdleSample {
                    idle,
                    limit,
                    read_at: Instant::now(),
                })) {
                    return;
                }
            }
            // Another operation holds the cloud: a skipped check, which neither fails
            // nor clears a failure, so the next check decides.
            Err(Error::Busy) => {}
            // No longer ready: if that is a stop this watch began and could not
            // finish, the card is shown it before the watch ends.
            Ok(IdleCheck::NotWatched) => {
                if let Some(state) = stopping(&load, interval) {
                    report(unfinished(state, "the server may still run"));
                    return;
                }
                // A ready worker that stops itself but keeps no idle record, as an older
                // image, is still read: a read that fails asks the provider.
                if load().ok().flatten().as_ref().is_some_and(stops_itself_when_ready) {
                    failed = None;
                    continue;
                }
                return;
            }
            Ok(IdleCheck::Stopped { idle }) => {
                // Ends this watch's presentation of the released server itself, so the
                // card never has to cancel a token a newer operation may own.
                cancel.cancel();
                let line = format!(
                    "No agent activity for {} minutes, so Horizon stopped this cloud. \
                     Resume creates a new server that attaches the same workspace volume.",
                    idle.as_secs() / 60
                );
                // The stop just released the lock; only another operation can hold it
                // now. Only the stopped record is this stop's; the stop is reported
                // either way, so the card never stays ready over a released server.
                let state = (0..30).find_map(|_| {
                    load()
                        .ok()
                        .flatten()
                        .filter(|state| state.stage == Stage::Stopped)
                        .or_else(|| {
                            std::thread::sleep(interval.min(Duration::from_secs(1)));
                            None
                        })
                });
                let line = if state.is_some() {
                    line
                } else {
                    format!("{line} Its record could not be read back; choose Check provider to show it.")
                };
                report(Report::Stopped(state.map(Box::new), line));
                return;
            }
            Err(error) => {
                // A stop that began and then failed leaves the record stopping and no
                // longer watched: show it, so the card offers to finish the stop.
                if let Some(state) = stopping(&load, interval) {
                    report(unfinished(state, &error.to_string()));
                    return;
                }
                // A worker that stopped itself cannot be read either.
                if let Ok(Some(state)) = confirm(cancel)
                    && !cancel.is_cancelled()
                {
                    cancel.cancel();
                    report(Report::StoppedOutside(Box::new(state)));
                    return;
                }
                let message = format!("Idle check failed: {error}");
                if failed.as_ref() != Some(&message) && !report(Report::Failed(message.clone())) {
                    return;
                }
                failed = Some(message);
            }
        }
    }
}

/// A ready cloud whose worker applies its idle period itself.
fn stops_itself_when_ready(state: &Deployment) -> bool {
    Description::of(&state.profile).idle_stop == IdleStop::Worker
        && state.profile.idle_stop_minutes.is_some()
        && state.stage == Stage::Ready
        && !state.stop_requested
}

/// The saved record when it shows a stop in progress, read with the same retries
/// as a finished stop's record, since another operation may hold it briefly.
fn stopping(load: &impl Fn() -> cloud_runtime::Result<Option<Deployment>>, interval: Duration) -> Option<Deployment> {
    (0..30).find_map(|_| {
        if let Ok(state) = load() {
            return Some(state.filter(|state| state.stage == Stage::Stopping));
        }
        std::thread::sleep(interval.min(Duration::from_secs(1)));
        None
    })?
}

/// The saved record once the lock an operation found busy is released, as when
/// Reconnect is chosen while an idle stop shuts the server down: the stop holds the
/// lock for a few seconds, and the card should then show what it left rather than
/// the busy failure. `None` when the lock stays held or the record cannot be read.
pub(super) fn after_busy(
    load: impl Fn() -> cloud_runtime::Result<Option<Deployment>>,
    pause: Duration,
) -> Option<Deployment> {
    (0..30)
        .find_map(|attempt| {
            if attempt > 0 {
                std::thread::sleep(pause);
            }
            match load() {
                Err(Error::Busy) => None,
                other => Some(other.ok().flatten()),
            }
        })
        .flatten()
}

/// What an operation that found the record busy reports once it could read it again:
/// a finished stop is shown as the card shows any Stop, and a stop that did not
/// finish offers to finish it. Any other record comes back, and the failure stands.
pub(super) fn stopped_while_busy(state: Deployment) -> Result<Vec<cloud_runtime::Event>, Box<Deployment>> {
    use cloud_runtime::Event;
    match state.stage {
        Stage::Stopped => Ok(vec![
            Event::Output(
                "This cloud was stopped, as by its idle stop, while this operation waited for it. \
                 Choose Resume worker to start it again."
                    .into(),
            ),
            Event::Stopped(Box::new(state)),
        ]),
        Stage::Stopping => Ok(vec![
            Event::Snapshot(Box::new(state)),
            Event::failed("A stop of this cloud did not finish; choose Reconcile stop to finish it.".into()),
        ]),
        _ => Err(Box::new(state)),
    }
}

/// A stop that began and did not finish, for the card to offer finishing it.
fn unfinished(state: Deployment, reason: &str) -> Report {
    Report::Stopped(
        Some(Box::new(state)),
        format!("Horizon's idle stop did not finish ({reason}); choose Reconcile stop to finish it."),
    )
}

/// The line logged for a stop Horizon did not make, which says why when it can.
fn outside_line(state: &Deployment, cause: StopCause) -> String {
    let resume = match Description::of(&state.profile).stopped {
        StoppedCost::WorkerKept => "Resume starts the same worker again.",
        StoppedCost::ServerDeleted => "Resume creates a new server that attaches the same workspace volume.",
    };
    match cause {
        StopCause::Idle { limit } => format!(
            "No agent activity for {} minutes, so this worker stopped itself. \
             The provider confirmed that it is stopped. {resume}",
            limit.as_secs() / 60
        ),
        StopCause::Unknown => format!(
            "The provider reports that this worker is stopped. Horizon did not stop it; \
             an agent on the worker or the provider account may have. {resume}"
        ),
    }
}

/// Sleeps for `interval` in short steps; false once `cancel` is cancelled.
fn wait(cancel: &Cancellation, interval: Duration) -> bool {
    let step = Duration::from_millis(250).min(interval);
    let mut waited = Duration::ZERO;
    while waited < interval {
        if cancel.is_cancelled() {
            return false;
        }
        std::thread::sleep(step);
        waited += step;
    }
    !cancel.is_cancelled()
}

impl Runtime {
    /// Listens for the idle watch of the operation starting now; an earlier watch's
    /// reports, and the idle record it read, are dropped with its channel.
    pub(super) fn listen_idle(&mut self) -> Sender<Report> {
        let (reports, received) = std::sync::mpsc::channel();
        self.idle_reports = Some(received);
        self.last_idle = None;
        reports
    }

    /// Shows a stop the idle watch made as the card shows any finished Stop.
    pub(super) fn poll_idle(&mut self) {
        loop {
            let Some(reports) = &self.idle_reports else {
                return;
            };
            let report = match reports.try_recv() {
                Ok(report) => report,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.idle_reports = None;
                    return;
                }
            };
            self.show_idle(report);
        }
    }

    fn show_idle(&mut self, report: Report) {
        match report {
            Report::Checked => {}
            Report::Sampled(sample) => self.last_idle = Some(sample),
            Report::Failed(message) => self.push_note(message),
            // Only the watch of the current operation reports here: every new
            // operation replaces or drops the channel, and the watch already ended
            // its presentation, whose token is the one this card holds.
            Report::Stopped(state, line) => {
                self.show_stopped(state.map(|state| *state), line);
                // Horizon's own idle stop; a stop it could not finish has no cause yet.
                self.stop_cause = self
                    .state
                    .as_ref()
                    .filter(|_| self.stage == Some(Stage::Stopped))
                    .and_then(|state| state.profile.idle_stop_minutes)
                    .map(|minutes| StopCause::Idle {
                        limit: Duration::from_secs(u64::from(minutes) * 60),
                    });
            }
            Report::StoppedOutside(state) => self.show_stopped_outside(*state),
        }
    }

    /// Shows a stop the provider confirmed and Horizon did not make, with its cause
    /// as the newest idle record tells it.
    /// A record that was already stopped before the check, as by an earlier Stop,
    /// shows as that stop.
    pub(super) fn show_stopped_outside(&mut self, state: Deployment) {
        if self.state.as_ref().is_some_and(|shown| shown.stop_requested) {
            self.show_stopped(
                Some(state),
                "The provider confirmed that this worker is stopped. Choose Resume worker to start it again.".into(),
            );
            self.stop_cause = None;
            return;
        }
        let cause = StopCause::of(self.last_idle.as_ref(), Instant::now());
        let line = outside_line(&state, cause);
        self.show_stopped(Some(state), line);
        self.stop_cause = Some(cause);
    }

    fn show_stopped(&mut self, state: Option<Deployment>, line: String) {
        // A provider check Horizon started for a failure has nothing left to explain. It
        // ends early, and the card waits for it to release the cloud.
        if let Some(check) = &self.failure_check {
            check.cancel();
            self.unexplained_failure = None;
        }
        self.cancel = None;
        self.progress.stage(Stage::Stopped, Instant::now());
        if let Some(state) = state {
            self.stage = Some(state.stage);
            self.state = Some(state);
        } else {
            // Without the saved record, the one shown is marked stopped and its
            // released server forgotten, so nothing connects to it.
            self.stage = Some(Stage::Stopped);
            if let Some(state) = &mut self.state {
                state.stage = Stage::Stopped;
                state.stop_requested = true;
                state.worker = None;
            }
        }
        self.desktop = None;
        self.error = None;
        self.receiver = None;
        self.idle_reports = None;
        self.push_note(line);
    }
}

#[cfg(test)]
mod tests;
