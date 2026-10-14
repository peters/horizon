use crate::app::cloud_panel::production::{Deployment, Runtime, Stage, cloud_runtime};
use cloud_runtime::{Cancellation, Error};
use std::{cell::RefCell, sync::mpsc::channel, time::Duration};

fn stopped() -> Deployment {
    serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "cloud-1", "repository": "/fixture", "revision": "a".repeat(40),
        "profile": {"provider": "hetzner", "image": "registry.example/worker", "cpu": 4, "memory_gb": 8,
            "storage": {"container_gb": 20, "volume_gb": 50}},
        "stage": "Stopped", "operation": {"state": "bound", "worker_id": "42"},
        "worker": null, "sessions": [], "stop_requested": true
    }))
    .unwrap()
}

/// A `RunPod` cloud whose worker stopped itself, as the provider check records it.
fn runpod_stopped() -> Deployment {
    let mut state = stopped();
    state.profile.provider = "runpod".into();
    state
}

#[test]
fn an_operation_that_found_the_record_busy_reads_it_once_the_lock_is_released() {
    // Busy twice, as while another stop still holds the lock, then the stopped record.
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
fn a_reconnect_that_met_another_stop_shows_the_stop_instead_of_the_busy_failure() {
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
fn a_busy_failure_waits_for_the_other_stop_and_reports_the_stopped_cloud() {
    use cloud_runtime::{Event, state::Store};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_owned();
    // Another stop holds the record while the reconnect fails on it.
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

#[test]
fn a_stop_the_provider_confirmed_is_shown_as_a_stop_outside_horizon() {
    let mut runtime = Runtime {
        stage: Some(Stage::Ready),
        ..Runtime::default()
    };
    runtime.show_stopped_outside(runpod_stopped());
    assert_eq!(runtime.stage, Some(Stage::Stopped));
    assert!(runtime.stopped_outside);
    let note = &runtime.logs.back().unwrap().text;
    assert!(
        note.contains("Horizon did not stop it") && note.ends_with("Resume starts the same worker again."),
        "{note}"
    );
    // A record the card already showed stopped keeps the words of that stop.
    let mut runtime = Runtime {
        stage: Some(Stage::Stopped),
        state: Some(runpod_stopped()),
        ..Runtime::default()
    };
    runtime.show_stopped_outside(runpod_stopped());
    assert!(!runtime.stopped_outside);
}

/// The provider check of a `RunPod` worker in `status`, as it records the cloud.
fn checked(status: &str) -> cloud_runtime::lifecycle::ReconciledDeployment {
    let mut state = runpod_stopped();
    state.worker = serde_json::from_value(serde_json::json!({
        "id": "42", "name": "cloud-1", "imageName": "registry.example/worker", "desiredStatus": status
    }))
    .unwrap();
    if status != "EXITED" {
        (state.stage, state.stop_requested) = (Stage::Ready, false);
    }
    let worker = state.worker.clone();
    let mut reconciled = cloud_runtime::lifecycle::ReconciledDeployment {
        state,
        report: serde_json::from_value(serde_json::json!({
            "operation_id": "cloud-1", "outcome": {"status": "inactive", "worker_id": "42"}
        }))
        .unwrap(),
    };
    reconciled.report.worker = worker;
    reconciled
}

// Each failure text one stopped pod gave in the live run (#1321), on the path that gave
// it, becomes the stopped cloud with Resume worker once the provider confirms it.
#[test]
fn every_failure_of_a_worker_stopped_outside_horizon_becomes_its_stop() {
    use std::time::Instant;
    let ready = || {
        let mut state = runpod_stopped();
        (state.stage, state.stop_requested) = (Stage::Ready, false);
        state
    };
    let resumed = Some(super::super::lifecycle::Action::Resume);
    let cases = [
        // The connection watch of a ready cloud timed out.
        ("Local operation timed out", Stage::Ready, None, ready()),
        // The connection watch of a cloud that Resume worker reconnected lost the worker.
        (
            "Cloud presentation discovery failed; inspect deployment output",
            Stage::Ready,
            resumed,
            ready(),
        ),
        // A reconnect found the worker not running.
        (
            "Existing worker is not running; check provider before reconnecting",
            Stage::Provision,
            None,
            ready(),
        ),
        // A reconnect found the record that a provider check had already stopped.
        (
            "Worker was explicitly stopped. Resume it before reconnecting; prior processes may be lost.",
            Stage::Stopped,
            None,
            runpod_stopped(),
        ),
    ];
    for (error, stage, operation, state) in cases {
        let (_events, deploy) = channel();
        let mut runtime = Runtime {
            receiver: Some(deploy),
            cancel: Some(Cancellation::default()),
            state: Some(state),
            ..Runtime::default()
        };
        runtime.progress.stage(Stage::Validate, Instant::now());
        if stage == Stage::Ready {
            runtime.progress.stage(Stage::Ready, Instant::now());
        }
        runtime.stage = Some(stage);
        runtime.operation = operation;
        runtime.show_failure(error.into(), Instant::now());
        assert!(runtime.failure_needs_check, "{error}");
        runtime.receiver = None;
        let check = runtime.hold_failure_for_check();
        assert!(runtime.checking_provider() && runtime.error.is_none(), "{error}");
        check.send(Ok(checked("EXITED"))).unwrap();
        runtime.poll_recovery();
        assert_eq!(runtime.stage, Some(Stage::Stopped), "{error}");
        assert!(runtime.error.is_none() && !runtime.busy(), "{error}");
        // A record already stopped keeps its own words; the others are stops outside Horizon.
        assert_eq!(runtime.stopped_outside, stage != Stage::Stopped, "{error}");
    }
    // The same failure of a worker that still runs stays the failure.
    let (_events, deploy) = channel();
    let mut runtime = Runtime {
        receiver: Some(deploy),
        state: Some(ready()),
        stage: Some(Stage::Provision),
        ..Runtime::default()
    };
    runtime.show_failure("Existing worker is not running".into(), Instant::now());
    let check = runtime.hold_failure_for_check();
    check.send(Ok(checked("RUNNING"))).unwrap();
    runtime.poll_recovery();
    assert_eq!(runtime.error.as_deref(), Some("Existing worker is not running"));
}

// The check of a failure reads the settings on its own thread: a read that does not
// return holds the check, not the card, and a failed read shows the failure as it was.
#[cfg(unix)]
#[test]
fn a_held_settings_read_does_not_hold_the_card_and_a_failed_read_keeps_the_failure() {
    use std::time::Instant;
    let temp = tempfile::tempdir().unwrap();
    let settings = temp.path().join("settings.json");
    // Opening a FIFO for reading waits until a writer opens it.
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&settings)
            .status()
            .unwrap()
            .success()
    );
    let mut runtime = Runtime {
        stage: Some(Stage::Ready),
        error: Some("Local operation timed out".into()),
        ..Runtime::default()
    };
    runtime.start_failure_check(temp.path().join("cloud-1"), settings.clone(), &egui::Context::default());
    assert!(runtime.checking_provider() && runtime.error.is_none());
    runtime.poll_recovery();
    assert!(runtime.checking_provider(), "the read is still held");
    // The writer closes without writing: the read ends with no settings.
    drop(std::fs::OpenOptions::new().write(true).open(&settings).unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    while runtime.checking_provider() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        runtime.poll_recovery();
    }
    assert!(!runtime.checking_provider());
    assert_eq!(runtime.error.as_deref(), Some("Local operation timed out"));
}
