//! Idle stop for clouds whose workers cannot stop themselves (see
//! `provider::IdleStop::Horizon`): while such a cloud is ready, Horizon reads its
//! worker's idle record every few minutes and stops the cloud after its idle
//! period, as Stop does. The watch has its own channel, so a presentation failure
//! on the released server can never hide the stop.
use super::{Deployment, Runtime, Settings, Stage, Store, cloud_runtime};
use cloud_runtime::{Cancellation, Error, lifecycle::IdleCheck, provider::IdleStop};
use std::{
    path::Path,
    sync::mpsc::{Receiver, Sender, TryRecvError},
    time::Duration,
};

/// How often the idle record is read. The worker rewrites it every minute.
const CHECK_INTERVAL: Duration = Duration::from_secs(120);

/// What the idle watch tells the cloud's card.
pub(super) enum Report {
    /// Horizon stopped the cloud, or began to and must be asked to finish; the
    /// saved record, when it could be read back, and the line to log.
    Stopped(Option<Box<Deployment>>, String),
    /// A check failed; logged once until the failure changes.
    Failed(String),
    /// A check is about to run. Sent only to learn whether the card still listens,
    /// so a watch whose card is gone, as after a session switch, ends before it
    /// reads the record or stops anything.
    Checked,
}

pub(super) type Reports = Receiver<Report>;

/// Whether Horizon, not the worker, stops this cloud when it is idle.
fn watched(state: &Deployment) -> bool {
    state.profile.idle_stop_minutes.is_some()
        && cloud_runtime::provider::by_id(&state.profile.provider)
            .is_some_and(|provider| provider.idle_stop == IdleStop::Horizon)
}

/// Starts the idle watch of a ready cloud Horizon stops. It ends with `cancel`,
/// once the cloud is no longer watched, or when its card stops listening.
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
            &|report| {
                let repaint = !matches!(report, Report::Checked);
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
/// `report` finds nobody listening.
fn run(
    cancel: &Cancellation,
    interval: Duration,
    check: impl Fn(&Cancellation) -> cloud_runtime::Result<IdleCheck>,
    load: impl Fn() -> cloud_runtime::Result<Option<Deployment>>,
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
            Ok(IdleCheck::Active { .. }) => failed = None,
            // Another operation holds the cloud: a skipped check, which neither fails
            // nor clears a failure, so the next check decides.
            Err(Error::Busy) => {}
            // No longer ready: if that is a stop this watch began and could not
            // finish, the card is shown it before the watch ends.
            Ok(IdleCheck::NotWatched) => {
                if let Some(state) = stopping(&load, interval) {
                    report(unfinished(state, "the server may still run"));
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
                let message = format!("Idle check failed: {error}");
                if failed.as_ref() != Some(&message) && !report(Report::Failed(message.clone())) {
                    return;
                }
                failed = Some(message);
            }
        }
    }
}

/// The saved record when it shows a stop in progress, read with the same retries
/// as a finished stop's record, since another operation may hold it briefly.
fn stopping(load: &impl Fn() -> cloud_runtime::Result<Option<Deployment>>, interval: Duration) -> Option<Deployment> {
    (0..30).find_map(|_| match load() {
        Ok(state) => Some(state.filter(|state| state.stage == Stage::Stopping)),
        Err(_) => {
            std::thread::sleep(interval.min(Duration::from_secs(1)));
            None
        }
    })?
}

/// A stop that began and did not finish, for the card to offer finishing it.
fn unfinished(state: Deployment, reason: &str) -> Report {
    Report::Stopped(
        Some(Box::new(state)),
        format!("Horizon's idle stop did not finish ({reason}); choose Reconcile stop to finish it."),
    )
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
    /// reports are dropped with its channel.
    pub(super) fn listen_idle(&mut self) -> Sender<Report> {
        let (reports, received) = std::sync::mpsc::channel();
        self.idle_reports = Some(received);
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
            Report::Failed(message) => self.push_log(message),
            // Only the watch of the current operation reports here: every new
            // operation replaces or drops the channel, and the watch already ended
            // its presentation, whose token is the one this card holds.
            Report::Stopped(state, line) => {
                self.cancel = None;
                self.progress.stage(Stage::Stopped, std::time::Instant::now());
                if let Some(state) = state {
                    self.stage = Some(state.stage);
                    self.state = Some(*state);
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
                self.push_log(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Report, run};
    use crate::app::cloud_panel::production::{Deployment, Runtime, Stage, cloud_runtime};
    use cloud_runtime::{Cancellation, Error, lifecycle::IdleCheck};
    use std::{cell::RefCell, sync::mpsc::channel, time::Duration};

    fn stopped() -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version": 1, "cloud_id": "cloud-1", "repository": "/fixture", "revision": "a".repeat(40),
            "profile": {"provider": "hetzner", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
                "storage": {"container_gb": 20, "volume_gb": 50}, "idle_stop_minutes": 10},
            "stage": "Stopped", "operation": {"state": "bound", "worker_id": "42"},
            "worker": null, "sessions": [], "stop_requested": true
        }))
        .unwrap()
    }

    /// Runs the watch over `results`, one per check, and returns what it reported.
    fn watch(results: Vec<cloud_runtime::Result<IdleCheck>>) -> Vec<String> {
        watch_with(&Cancellation::default(), results)
    }

    fn watch_with(cancel: &Cancellation, results: Vec<cloud_runtime::Result<IdleCheck>>) -> Vec<String> {
        let results = RefCell::new(results.into_iter());
        let reports = RefCell::new(Vec::new());
        run(
            cancel,
            Duration::ZERO,
            |_| results.borrow_mut().next().unwrap_or(Ok(IdleCheck::NotWatched)),
            || Ok(Some(stopped())),
            &|report| {
                reports.borrow_mut().push(match report {
                    Report::Failed(message) => message,
                    Report::Stopped(state, line) => format!("{:?}: {line}", state.map(|state| state.stage)),
                    Report::Checked => return true,
                });
                true
            },
        );
        reports.into_inner()
    }

    #[test]
    fn a_cloud_idle_for_its_period_is_reported_stopped_once() {
        let active = || {
            Ok(IdleCheck::Active {
                idle: Duration::from_secs(60),
                limit: Duration::from_secs(600),
            })
        };
        let cancel = Cancellation::default();
        let reports = watch_with(
            &cancel,
            vec![
                active(),
                Err(Error::Busy),
                active(),
                Ok(IdleCheck::Stopped {
                    idle: Duration::from_mins(11),
                }),
                active(),
            ],
        );
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(
            cancel.is_cancelled(),
            "the watch ends its own presentation of the released server"
        );
        assert!(reports[0].starts_with("Some(Stopped): No agent activity for 11 minutes"));
    }

    #[test]
    fn a_failure_is_logged_once_until_it_changes_and_a_cloud_no_longer_watched_ends_the_watch() {
        let reports = watch(vec![
            Err(Error::Invalid("The worker's idle record is malformed")),
            // A skipped check between two equal failures logs nothing new.
            Err(Error::Busy),
            Err(Error::Invalid("The worker's idle record is malformed")),
            Err(Error::Command("Reading the worker's idle record")),
            Ok(IdleCheck::NotWatched),
            Err(Error::Invalid("never checked")),
        ]);
        assert_eq!(
            reports,
            [
                "Idle check failed: The worker's idle record is malformed",
                "Idle check failed: Reading the worker's idle record failed; inspect deployment output",
            ]
        );
    }

    #[test]
    fn a_watch_whose_card_stopped_listening_checks_nothing_more() {
        let checks = RefCell::new(0);
        run(
            &Cancellation::default(),
            Duration::ZERO,
            |_| {
                *checks.borrow_mut() += 1;
                Ok(IdleCheck::Active {
                    idle: Duration::ZERO,
                    limit: Duration::from_secs(600),
                })
            },
            || panic!("nothing to load"),
            // The card listened for the first check only.
            &|report| matches!(report, Report::Checked) && *checks.borrow() == 0,
        );
        assert_eq!(*checks.borrow(), 1);
    }

    #[test]
    fn a_cancelled_watch_checks_nothing() {
        let cancel = Cancellation::default();
        cancel.cancel();
        run(
            &cancel,
            Duration::ZERO,
            |_| panic!("no check after cancellation"),
            || panic!("nothing to load"),
            &|_| panic!("nothing to report"),
        );
    }

    #[test]
    fn the_card_shows_an_idle_stop_as_a_finished_stop() {
        let (reports, received) = channel();
        let (_events, deploy) = channel();
        let cancel = Cancellation::default();
        let mut runtime = Runtime {
            idle_reports: Some(received),
            receiver: Some(deploy),
            cancel: Some(cancel.clone()),
            stage: Some(Stage::Ready),
            ..Runtime::default()
        };
        reports.send(Report::Failed("Idle check failed: busy".into())).unwrap();
        runtime.poll_idle();
        assert_eq!(runtime.stage, Some(Stage::Ready));
        reports
            .send(Report::Stopped(Some(Box::new(stopped())), "stopped when idle".into()))
            .unwrap();
        runtime.poll_idle();
        assert_eq!(runtime.stage, Some(Stage::Stopped));
        // The watch ended its presentation; the card only lets go of the token.
        assert!(runtime.cancel.is_none() && !cancel.is_cancelled());
        assert!(runtime.receiver.is_none() && runtime.idle_reports.is_none() && runtime.error.is_none());
        assert_eq!(
            runtime.logs.iter().rev().take(2).collect::<Vec<_>>(),
            ["stopped when idle", "Idle check failed: busy"]
        );
    }

    #[test]
    fn a_stop_whose_record_cannot_be_read_back_is_still_reported() {
        for load in [
            (|| Err(Error::Busy)) as fn() -> cloud_runtime::Result<Option<Deployment>>,
            // Another controller already moved the cloud on: not this stop's record.
            || {
                let mut resumed = stopped();
                resumed.stage = Stage::Readiness;
                Ok(Some(resumed))
            },
        ] {
            let reports = RefCell::new(Vec::new());
            run(
                &Cancellation::default(),
                Duration::ZERO,
                |_| {
                    Ok(IdleCheck::Stopped {
                        idle: Duration::from_mins(10),
                    })
                },
                load,
                &|report| {
                    if let Report::Stopped(state, line) = report {
                        reports.borrow_mut().push((state.is_some(), line));
                    }
                    true
                },
            );
            let reports = reports.into_inner();
            assert_eq!(reports.len(), 1);
            assert!(!reports[0].0 && reports[0].1.ends_with("choose Check provider to show it."));
        }
        // The card shows it stopped, and the record it had no longer reads as ready.
        let (sender, received) = channel();
        let mut ready = stopped();
        (ready.stage, ready.stop_requested) = (Stage::Ready, false);
        let mut runtime = Runtime {
            idle_reports: Some(received),
            state: Some(ready),
            stage: Some(Stage::Ready),
            ..Runtime::default()
        };
        sender.send(Report::Stopped(None, "stopped".into())).unwrap();
        runtime.poll_idle();
        assert_eq!(runtime.stage, Some(Stage::Stopped));
        let state = runtime.state.as_ref().unwrap();
        assert!(state.stage == Stage::Stopped && state.stop_requested && state.worker.is_none());
    }

    #[test]
    fn a_stop_left_unfinished_is_shown_even_after_a_busy_reload() {
        // The failed stop's reload finds the record busy; the next check sees it stopping.
        let loads = RefCell::new(0);
        let checks = RefCell::new(
            vec![
                Err(Error::Command("Hetzner server shutdown")),
                Ok(IdleCheck::NotWatched),
            ]
            .into_iter(),
        );
        let reports = RefCell::new(Vec::new());
        run(
            &Cancellation::default(),
            Duration::ZERO,
            |_| checks.borrow_mut().next().unwrap(),
            || {
                *loads.borrow_mut() += 1;
                if *loads.borrow() <= 30 {
                    return Err(Error::Busy);
                }
                let mut stopping = stopped();
                stopping.stage = Stage::Stopping;
                Ok(Some(stopping))
            },
            &|report| {
                match report {
                    Report::Stopped(state, _) => reports.borrow_mut().push(state.map(|state| state.stage)),
                    Report::Failed(_) => reports.borrow_mut().push(None),
                    Report::Checked => {}
                }
                true
            },
        );
        assert_eq!(
            reports.into_inner(),
            [None, Some(Stage::Stopping)],
            "logged, then shown"
        );
    }

    #[test]
    fn a_stop_that_began_and_failed_is_shown_for_the_card_to_finish() {
        let reports = RefCell::new(Vec::new());
        run(
            &Cancellation::default(),
            Duration::ZERO,
            |_| Err(Error::Command("Hetzner server shutdown")),
            || {
                let mut stopping = stopped();
                stopping.stage = Stage::Stopping;
                Ok(Some(stopping))
            },
            &|report| {
                if let Report::Stopped(state, line) = report {
                    reports.borrow_mut().push((state.map(|state| state.stage), line));
                }
                true
            },
        );
        let reports = reports.into_inner();
        assert_eq!(reports.len(), 1, "reported once, and the watch ends");
        assert_eq!(reports[0].0, Some(Stage::Stopping));
        assert!(reports[0].1.contains("choose Reconcile stop"));
    }

    #[test]
    fn a_stop_reported_after_a_newer_operation_began_is_never_applied() {
        let (reports, received) = channel();
        let newer = Cancellation::default();
        let mut runtime = Runtime {
            idle_reports: Some(received),
            cancel: Some(newer.clone()),
            stage: Some(Stage::Stopping),
            ..Runtime::default()
        };
        // What starting Stop, Delete or Resume does to the card's idle watch.
        runtime.idle_reports = None;
        assert!(
            reports
                .send(Report::Stopped(Some(Box::new(stopped())), "late".into()))
                .is_err(),
            "the late watch finds nobody listening"
        );
        runtime.poll_idle();
        assert_eq!(runtime.stage, Some(Stage::Stopping));
        assert!(runtime.cancel.is_some() && !newer.is_cancelled());
    }
}
