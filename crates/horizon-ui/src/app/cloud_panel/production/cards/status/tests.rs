use super::super::super::{LogLine, Runtime, Stage};
use super::*;
use horizon_core::cloud_runtime;
use horizon_core::cloud_runtime::progress::{Progress, Unit};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

fn deployment(
    stage: &str,
    operation: &serde_json::Value,
    worker: &serde_json::Value,
) -> cloud_runtime::state::Deployment {
    serde_json::from_value(serde_json::json!({
        "version": 1, "cloud_id": "status", "repository": "/synthetic", "revision": "a",
        "profile": {"provider": "runpod", "image": "registry.example/worker", "cpu": 8, "memory_gb": 32},
        "stage": stage, "operation": operation, "spec": null, "sessions": [], "worker": worker,
        "ready_after_seconds": 252
    }))
    .unwrap()
}

fn bound() -> serde_json::Value {
    serde_json::json!({"state": "bound", "worker_id": "k3x9"})
}

fn running_worker() -> serde_json::Value {
    serde_json::json!({"id": "k3x9", "name": "status", "imageName": "registry.example/worker",
        "desiredStatus": "RUNNING", "costPerHr": 0.32, "lastStartedAt": "2026-09-28T09:00:00Z"})
}

/// A runtime with a live operation; keep the sender so the receiver stays connected.
fn live(stage: Stage) -> (Runtime, Sender<cloud_runtime::Event>) {
    let (sender, receiver): (_, Receiver<_>) = channel();
    let mut runtime = Runtime {
        stage: Some(stage),
        receiver: Some(receiver),
        cancel: Some(cloud_runtime::Cancellation::default()),
        ..Runtime::default()
    };
    runtime.progress.stage(Stage::Validate, Instant::now());
    runtime.progress.stage(stage, Instant::now());
    (runtime, sender)
}

fn line(text: &str, stage: Stage) -> LogLine {
    LogLine::new(text.into(), Some(stage), None)
}

fn now() -> SystemTime {
    SystemTime::now()
}

#[test]
fn a_new_cloud_offers_deploy_and_bills_nothing() {
    let status = of(&Runtime::default(), Occupancy::default(), now());
    assert_eq!(status.verb, "Not deployed");
    assert_eq!(status.tone, Tone::Idle);
    assert_eq!(status.primary, Some(Primary::Deploy));
    assert_eq!(status.track.current, None);
    assert_eq!(status.track.finished, 0);
    assert!(status.failure.is_none());
}

#[test]
fn a_push_reports_bytes_rate_position_and_cancel() {
    let (mut runtime, _sender) = live(Stage::Push);
    let start = Instant::now();
    for (seconds, bytes) in [(0, 0_u64), (2, 200_000_000)] {
        runtime.progress.update(Progress {
            detail: "7/16 layers complete · Docker-reported bytes".into(),
            observed_at: start + Duration::from_secs(seconds),
            completed: bytes,
            total: Some(1_000_000_000),
            unit: Unit::Bytes,
            transferred: Some(bytes),
        });
    }
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Pushing image");
    assert_eq!(status.tone, Tone::Live);
    assert!(status.numbers.starts_with("200.0 MB / 1.0 GB"), "{}", status.numbers);
    assert!(status.numbers.contains("/s"), "{}", status.numbers);
    assert!(
        status.tail.starts_with('~') && status.tail.ends_with("left"),
        "{}",
        status.tail
    );
    assert!(status.right.starts_with("Stage 3/8"), "{}", status.right);
    assert_eq!(status.primary, Some(Primary::Cancel));
    assert_eq!(status.track.current, Some(2));
    assert_eq!(status.track.finished, 2);
    assert!((status.track.fraction.unwrap() - 0.2).abs() < 0.01);
}

#[test]
fn unmeasured_steps_say_what_is_happening_instead_of_a_blank() {
    let (mut runtime, _sender) = live(Stage::Provision);
    runtime
        .progress
        .update(Progress::activity("Requesting or reconciling worker capacity"));
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Requesting worker");
    assert_eq!(status.numbers, "Requesting or reconciling worker capacity");
    assert_eq!(status.tail, "", "no ETA claim without a measurable total");
    assert_eq!(status.track.fraction, None);
}

#[test]
fn a_failed_push_leads_with_the_registry_denial_and_what_it_means() {
    let mut runtime = Runtime {
        stage: Some(Stage::Push),
        error: Some("Uploading image failed; inspect deployment output".into()),
        ..Runtime::default()
    };
    runtime.progress.stage(Stage::Push, Instant::now());
    runtime.progress.finish(Instant::now());
    runtime.logs.extend([
        line("docker push registry.example/worker:9f3c2a1", Stage::Push),
        line("ceabf9021ae9: Waiting", Stage::Push),
        line("error from registry: denied", Stage::Push),
        line("Uploading image failed; inspect deployment output", Stage::Push),
    ]);
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Push failed");
    assert_eq!(status.tone, Tone::Failed);
    assert_eq!(status.numbers, "error from registry: denied");
    let failure = status.failure.as_ref().unwrap();
    assert!(failure.meaning.unwrap().contains("registry refused"));
    assert_eq!(
        failure.copy_text(),
        "error from registry: denied\nUploading image failed; inspect deployment output"
    );
    assert_eq!(status.right, "Stopped at stage 3/8 · output kept");
    assert!(status.track.failed);
    assert_eq!(status.primary, Some(Primary::Retry));
}

#[test]
fn a_failure_without_a_failure_line_keeps_horizons_summary() {
    let runtime = Runtime {
        stage: Some(Stage::Readiness),
        error: Some("Worker did not become ready".into()),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Readiness check failed");
    assert_eq!(status.numbers, "Worker did not become ready");
    assert_eq!(status.failure.unwrap().cause, None);
}

#[test]
fn a_cloud_that_was_ready_retries_by_reconnecting() {
    let runtime = Runtime {
        stage: Some(Stage::Readiness),
        error: Some("SSH readiness failed".into()),
        state: Some(deployment("Ready", &bound(), &running_worker())),
        ..Runtime::default()
    };
    assert_eq!(
        of(&runtime, Occupancy::default(), now()).primary,
        Some(Primary::Reconnect)
    );
    let failed_stop = Runtime {
        stage: Some(Stage::Stopping),
        error: Some("Stop request failed: provider timed out".into()),
        ..runtime
    };
    assert_eq!(
        of(&failed_stop, Occupancy::default(), now()).primary,
        Some(Primary::ReconcileStop),
        "a failed stop is confirmed, not answered by reconnecting the worker"
    );
}

#[test]
fn a_ready_cloud_counts_terminals_uptime_and_offers_stop() {
    let (mut runtime, _sender) = live(Stage::Ready);
    runtime.state = Some(deployment("Ready", &bound(), &running_worker()));
    let occupancy = Occupancy {
        panels: 3,
        running: 2,
        terminals: 3,
    };
    let status = of(&runtime, occupancy, now());
    assert_eq!(status.verb, "Ready");
    assert_eq!(status.tone, Tone::Ready);
    assert_eq!(status.numbers, "2/3 terminals running");
    assert!(status.tail.starts_with("up "), "{}", status.tail);
    assert_eq!(status.right, "Ready in 4m 12s · 3 panels");
    assert_eq!(status.primary, Some(Primary::Stop));
    assert_eq!(status.track.finished, status.track.stages.len());
    assert!(!status.track.faded);
    let empty = of(&runtime, Occupancy::default(), now());
    assert_eq!(empty.numbers, "No panels yet");
    assert_eq!(empty.right, "Ready in 4m 12s · No panels");
}

#[test]
fn stopped_and_stopping_clouds_offer_resume_and_reconcile() {
    let stopped = Runtime {
        stage: Some(Stage::Stopped),
        state: Some(deployment("Stopped", &bound(), &running_worker())),
        ..Runtime::default()
    };
    let status = of(&stopped, Occupancy::default(), now());
    assert_eq!(
        (status.verb.as_str(), status.primary),
        ("Stopped", Some(Primary::Resume))
    );
    assert_eq!(status.numbers, "Storage kept · billable");
    assert!(status.track.faded);
    let stopping = Runtime {
        stage: Some(Stage::Stopping),
        ..stopped
    };
    let status = of(&stopping, Occupancy::default(), now());
    assert_eq!(status.primary, Some(Primary::ReconcileStop));
}

#[test]
fn a_saved_cloud_that_is_not_connected_offers_reconnect() {
    let runtime = Runtime {
        stage: Some(Stage::Ready),
        state: Some(deployment("Ready", &bound(), &running_worker())),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Disconnected");
    assert_eq!(status.primary, Some(Primary::Reconnect));
    assert!(status.track.faded, "its last run is kept, faded");
}

#[test]
fn an_unreadable_record_offers_no_shortcut() {
    let runtime = Runtime {
        state_unavailable: true,
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.tone, Tone::Failed);
    assert_eq!(status.primary, None);
    assert_eq!(status.numbers, "Deployment state could not be read");
    assert!(
        !status.tail.contains("provider"),
        "a provider check needs the record too: {}",
        status.tail
    );
    let with_reason = Runtime {
        state_unavailable: true,
        error: Some("Deployment record is missing; reconcile its worker before continuing".into()),
        ..Runtime::default()
    };
    assert_eq!(
        of(&with_reason, Occupancy::default(), now()).numbers,
        "Deployment record is missing; reconcile its worker before continuing",
        "the restore's reason is the headline"
    );
}

#[test]
fn deletion_shows_its_own_steps_and_cancels_only_before_the_worker_delete() {
    let (mut runtime, _sender) = live(Stage::ReleaseDevices);
    runtime.progress.begin_deletion();
    runtime.progress.stage(Stage::ReleaseDevices, Instant::now());
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Deleting cloud resources");
    assert_eq!(status.track.stages, &Stage::DELETION);
    assert_eq!(status.primary, Some(Primary::Cancel));
    runtime.stage = Some(Stage::DeleteWorker);
    runtime.progress.stage(Stage::DeleteWorker, Instant::now());
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.primary, None, "a sent delete request cannot be recalled");
    assert_eq!(status.right.split(" · ").next(), Some("Stage 2/3"));
}

#[test]
fn a_deleted_cloud_offers_redeploy() {
    let runtime = Runtime {
        stage: Some(Stage::Deleted),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Worker deleted");
    assert_eq!(status.primary, Some(Primary::Redeploy));
}

#[test]
fn an_unconfirmed_worker_asks_for_a_provider_check() {
    let runtime = Runtime {
        stage: Some(Stage::Provision),
        state: Some(deployment(
            "Provision",
            &serde_json::json!({"state": "requested"}),
            &serde_json::Value::Null,
        )),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Needs provider check");
    assert_eq!(status.primary, Some(Primary::CheckProvider));
}

#[test]
fn a_rebuild_uses_its_own_steps() {
    let (mut runtime, _sender) = live(Stage::Replace);
    runtime.rebuild = Some(super::super::super::rebuild::Attempt {
        kind: super::super::super::rebuild::Kind::Rebuild,
        started: Instant::now(),
        notes: Vec::new(),
    });
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.track.stages.len(), 7);
    assert_eq!(status.verb, "Replacing image");
    assert_eq!(status.track.position().as_deref(), Some("Stage 4/7"));
    assert_eq!(
        status.primary, None,
        "the image switch cannot be cancelled, as Manage says"
    );
    runtime.stage = Some(Stage::Build);
    assert_eq!(of(&runtime, Occupancy::default(), now()).primary, Some(Primary::Cancel));
    runtime.rebuild.as_mut().unwrap().kind = super::super::super::rebuild::Kind::Cancel;
    assert_eq!(
        of(&runtime, Occupancy::default(), now()).primary,
        None,
        "a cancelling attempt is not cancelled again"
    );
}

#[test]
fn operations_manage_holds_come_before_a_generic_failure() {
    use horizon_core::cloud_runtime::deployment::ResizeTarget;
    let mut runtime = Runtime {
        stage: Some(Stage::Ready),
        error: Some("Resize failed".into()),
        state: Some(deployment("Ready", &bound(), &running_worker())),
        ..Runtime::default()
    };
    runtime.resize.pending = Some(ResizeTarget::Workspace { size_gb: 120 });
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(
        (status.verb.as_str(), status.primary),
        ("Resize pending", Some(Primary::Manage))
    );
    runtime.resize.pending = None;
    runtime.error = None;
    runtime.remote_release_error = Some("BrowserStack refused the release".into());
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Device release failed");
    assert_eq!(status.primary, Some(Primary::Manage), "not a deploy retry");
    assert_eq!(status.failure.unwrap().cause, None, "deploy output is not its cause");
    runtime.remote_release_error = None;
    let requested = Runtime {
        stage: Some(Stage::Provision),
        error: Some("Provider request timed out".into()),
        state: Some(deployment(
            "Provision",
            &serde_json::json!({"state": "requested"}),
            &serde_json::Value::Null,
        )),
        ..Runtime::default()
    };
    let status = of(&requested, Occupancy::default(), now());
    assert_eq!(
        status.primary,
        Some(Primary::CheckProvider),
        "Manage only offers the check"
    );
    assert_eq!(status.numbers, "Provider request timed out");
}

#[test]
fn a_connected_ready_cloud_stays_ready_when_a_follow_up_fails() {
    let (mut runtime, _sender) = live(Stage::Ready);
    runtime.state = Some(deployment("Ready", &bound(), &running_worker()));
    runtime.error = Some("Cloud settings could not be read".into());
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Ready");
    assert_eq!(status.primary, Some(Primary::Stop));
    assert_eq!(status.tail, "Cloud settings could not be read");
}

#[test]
fn the_cause_is_found_among_lines_held_while_a_reader_scrolled_up() {
    let mut runtime = Runtime {
        stage: Some(Stage::Push),
        error: Some("Uploading image failed; inspect deployment output".into()),
        ..Runtime::default()
    };
    runtime
        .logs
        .push_back(line("docker push registry.example/worker", Stage::Push));
    runtime
        .pending_logs
        .push_back(line("error from registry: denied", Stage::Push));
    let first = of(&runtime, Occupancy::default(), now());
    assert_eq!(first.numbers, "error from registry: denied");
    // A cached diagnosis is refreshed when the output changes.
    runtime
        .pending_logs
        .push_back(line("fatal: disk quota exceeded", Stage::Push));
    assert_eq!(
        of(&runtime, Occupancy::default(), now()).numbers,
        "fatal: disk quota exceeded"
    );
}

#[test]
fn every_primary_action_has_a_short_label() {
    for primary in [
        Primary::Deploy,
        Primary::Reconnect,
        Primary::Retry,
        Primary::Cancel,
        Primary::Resume,
        Primary::Stop,
        Primary::ReconcileStop,
        Primary::CheckProvider,
        Primary::Redeploy,
    ] {
        assert!(primary.label().chars().count() <= 15, "{}", primary.label());
    }
}

#[test]
fn a_check_core_reports_under_an_earlier_step_keeps_the_later_step_finished() {
    // The built image's contract is checked under Validate after Build.
    let start = Instant::now().checked_sub(Duration::from_secs(30)).unwrap();
    let mut runtime = Runtime {
        stage: Some(Stage::Validate),
        error: Some("worker image contract failed; inspect deployment output".into()),
        ..Runtime::default()
    };
    runtime.progress.stage(Stage::Validate, start);
    runtime.progress.stage(Stage::Build, start + Duration::from_secs(1));
    runtime.progress.stage(Stage::Validate, start + Duration::from_secs(21));
    runtime.progress.finish(start + Duration::from_secs(22));
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Validation failed");
    assert_eq!(status.track.current, Some(0));
    assert_eq!(status.track.finished, 2, "Build stays finished");
    assert_eq!(status.right, "Failed after Build locally · output kept");
}

#[test]
fn a_check_running_under_an_earlier_step_keeps_the_later_step_finished() {
    let start = Instant::now().checked_sub(Duration::from_secs(30)).unwrap();
    let (mut runtime, _sender) = live(Stage::Validate);
    runtime.progress.stage(Stage::Validate, start);
    runtime.progress.stage(Stage::Build, start + Duration::from_secs(1));
    runtime.progress.stage(Stage::Validate, start + Duration::from_secs(21));
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.track.current, Some(0));
    assert!(!status.track.failed);
    assert_eq!(status.track.finished, 2, "Build stays finished while the check runs");
}

#[test]
fn a_detail_that_repeats_the_verb_is_not_said_twice() {
    let (mut runtime, _sender) = live(Stage::Build);
    runtime
        .progress
        .update(Progress::activity("Building image · 1/3 reported steps complete"));
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Building image");
    assert_eq!(status.numbers, "1/3 reported steps complete");
}

#[test]
fn a_reconnected_cloud_says_how_long_the_reconnect_took() {
    let (mut runtime, _sender) = live(Stage::Ready);
    let mut state = deployment("Ready", &bound(), &running_worker());
    state.timeline = Some(cloud_runtime::timeline::Timeline {
        reconnected: true,
        spans: vec![cloud_runtime::timeline::Span {
            phase: cloud_runtime::timeline::Phase::Readiness,
            millis: 7_000,
        }],
        resume_requested: None,
    });
    runtime.state = Some(state);
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.right, "Reconnected in 0m 07s · No panels");
}

#[test]
fn a_half_deleted_cloud_waits_for_its_owner_and_offers_manage() {
    let runtime = Runtime {
        stage: Some(Stage::Stopped),
        state: Some(deployment(
            "Stopped",
            &serde_json::json!({"state": "terminated", "worker_id": "k3x9"}),
            &serde_json::Value::Null,
        )),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Worker deleted");
    assert_eq!(
        status.primary,
        Some(Primary::Manage),
        "the header leads to the cleanup controls"
    );
    assert_eq!(status.track.current, None, "cleanup is not running");
    let storage = Stage::DELETION
        .iter()
        .position(|stage| *stage == Stage::DeleteStorage)
        .unwrap();
    assert_eq!(status.track.finished, storage, "the steps before cleanup are done");
}

#[test]
fn a_resume_that_fails_before_the_provider_acts_offers_resume_again() {
    // The failed resume reloaded the still-stopped record before reporting.
    let runtime = Runtime {
        stage: Some(Stage::Stopped),
        resuming: true,
        error: Some("Provider API unavailable".into()),
        state: Some(deployment("Stopped", &bound(), &serde_json::Value::Null)),
        ..Runtime::default()
    };
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Resume failed");
    assert_eq!(status.primary, Some(Primary::Resume), "not a new deployment");
    let provision = Stage::ALL.iter().position(|stage| *stage == Stage::Provision).unwrap();
    assert_eq!(status.track.current, Some(provision), "the tried step is marked");
    assert!(status.track.failed);
}

#[test]
fn a_failed_deletion_leads_to_manage_where_deleting_again_is_confirmed() {
    let mut runtime = Runtime {
        error: Some("Worker deletion failed: provider timed out".into()),
        state: Some(deployment("Ready", &bound(), &running_worker())),
        ..Runtime::default()
    };
    runtime.progress.begin_deletion();
    let status = of(&runtime, Occupancy::default(), now());
    assert_eq!(status.verb, "Deletion failed");
    assert_eq!(status.primary, Some(Primary::Manage));
}
