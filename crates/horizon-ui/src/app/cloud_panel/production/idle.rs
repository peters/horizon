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
    /// Horizon stopped the cloud; the saved record and the line to log.
    Stopped(Box<Deployment>, String),
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
            Ok(IdleCheck::NotWatched) => return,
            Ok(IdleCheck::Stopped { idle }) => {
                let line = format!(
                    "No agent activity for {} minutes, so Horizon stopped this cloud. \
                     Resume creates a new server that attaches the same workspace volume.",
                    idle.as_secs() / 60
                );
                // The stop just released the lock; only another operation can hold it now.
                if let Some(state) = (0..30).find_map(|_| {
                    load().ok().flatten().or_else(|| {
                        std::thread::sleep(Duration::from_secs(1));
                        None
                    })
                }) {
                    report(Report::Stopped(Box::new(state), line));
                }
                return;
            }
            Err(error) => {
                let message = format!("Idle check failed: {error}");
                if failed.as_ref() != Some(&message) && !report(Report::Failed(message.clone())) {
                    return;
                }
                failed = Some(message);
            }
        }
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
            Report::Stopped(state, line) => {
                // Ends the presentation of the released server, as Stop does.
                if let Some(cancel) = self.cancel.take() {
                    cancel.cancel();
                }
                self.progress.stage(Stage::Stopped, std::time::Instant::now());
                self.stage = Some(state.stage);
                self.state = Some(*state);
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
        let results = RefCell::new(results.into_iter());
        let reports = RefCell::new(Vec::new());
        run(
            &Cancellation::default(),
            Duration::ZERO,
            |_| results.borrow_mut().next().unwrap_or(Ok(IdleCheck::NotWatched)),
            || Ok(Some(stopped())),
            &|report| {
                reports.borrow_mut().push(match report {
                    Report::Failed(message) => message,
                    Report::Stopped(state, line) => format!("{:?}: {line}", state.stage),
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
        let reports = watch(vec![
            active(),
            Err(Error::Busy),
            active(),
            Ok(IdleCheck::Stopped {
                idle: Duration::from_mins(11),
            }),
            active(),
        ]);
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].starts_with("Stopped: No agent activity for 11 minutes"));
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
            .send(Report::Stopped(Box::new(stopped()), "stopped when idle".into()))
            .unwrap();
        runtime.poll_idle();
        assert_eq!(runtime.stage, Some(Stage::Stopped));
        assert!(cancel.is_cancelled(), "the presentation of the released server ends");
        assert!(runtime.receiver.is_none() && runtime.idle_reports.is_none() && runtime.error.is_none());
        assert_eq!(
            runtime.logs.iter().rev().take(2).collect::<Vec<_>>(),
            ["stopped when idle", "Idle check failed: busy"]
        );
    }
}
