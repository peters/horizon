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
        runtime
            .logs
            .iter()
            .rev()
            .take(2)
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
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

#[test]
fn an_operation_that_found_the_record_busy_reads_it_once_the_lock_is_released() {
    // Busy twice, as while an idle stop still holds the lock, then the stopped record.
    let calls = RefCell::new(0);
    let loaded = super::after_busy(
        || {
            *calls.borrow_mut() += 1;
            if *calls.borrow() < 3 {
                Err(Error::Busy)
            } else {
                Ok(Some(stopped()))
            }
        },
        Duration::ZERO,
    );
    assert_eq!(loaded.map(|state| state.stage), Some(Stage::Stopped));
    assert_eq!(*calls.borrow(), 3);
    // A lock that stays held, or a record that cannot be read, gives nothing.
    let calls = RefCell::new(0);
    let held = super::after_busy(
        || {
            *calls.borrow_mut() += 1;
            Err(Error::Busy)
        },
        Duration::ZERO,
    );
    assert!(held.is_none());
    assert_eq!(*calls.borrow(), 30, "bounded");
    assert!(super::after_busy(|| Err(Error::Invalid("corrupt")), Duration::ZERO).is_none());
}

#[test]
fn a_reconnect_that_met_an_idle_stop_shows_the_stop_instead_of_the_busy_failure() {
    use cloud_runtime::Event;
    let events = super::stopped_while_busy(stopped()).ok().unwrap();
    assert!(matches!(&events[..], [Event::Output(line), Event::Stopped(state)]
        if line.contains("Resume worker") && state.stage == Stage::Stopped));
    // A stop that did not finish offers to finish it.
    let mut stopping = stopped();
    stopping.stage = Stage::Stopping;
    let events = super::stopped_while_busy(stopping).ok().unwrap();
    assert!(matches!(&events[..], [Event::Snapshot(state), Event::Failed(line, _)]
        if state.stage == Stage::Stopping && line.contains("Reconcile stop")));
    // Any other record keeps the failure.
    let mut ready = stopped();
    ready.stage = Stage::Ready;
    assert_eq!(
        super::stopped_while_busy(ready).err().map(|state| state.stage),
        Some(Stage::Ready)
    );
}

// Cloud records need a Unix host's durable directory updates.
#[cfg(unix)]
#[test]
fn a_busy_failure_waits_for_the_idle_stop_and_reports_the_stopped_cloud() {
    use cloud_runtime::{Event, state::Store};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_owned();
    // The idle stop holds the record while the reconnect fails on it.
    let stop = Store::lock(&root).unwrap();
    let (sender, receiver) = channel();
    let failing = std::thread::spawn({
        let root = root.clone();
        move || {
            super::super::report_failure(&root, &Error::Busy, &|event| {
                let _ = sender.send(event);
            });
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    stop.save(&stopped()).unwrap();
    drop(stop);
    failing.join().unwrap();
    let events: Vec<_> = receiver.try_iter().collect();
    assert!(
        matches!(&events[..], [Event::Output(_), Event::Stopped(state)] if state.stage == Stage::Stopped),
        "{events:?}"
    );
}
