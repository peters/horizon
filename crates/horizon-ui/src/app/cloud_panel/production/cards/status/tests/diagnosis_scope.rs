use super::*;
use crate::app::cloud_panel::production::rebuild::{Attempt, Kind};

const PLACEMENT: &str = "Provider rejected the request or capacity is unavailable: error during placement";

#[test]
fn a_replacement_server_refusal_does_not_blame_completed_image_steps() {
    let mut runtime = Runtime::default();
    for (stage, text) in [
        (
            Stage::Build,
            "#25 178.4 invoke-rc.d: policy-rc.d denied execution of start.",
        ),
        (Stage::Push, "error from registry: denied (retrying)"),
        (Stage::Replace, "error: transient warning from the released server"),
        (Stage::Provision, "Requesting"),
    ] {
        runtime.stage = Some(stage);
        runtime.progress.stage(stage, Instant::now());
        runtime.push_log(text.into());
    }
    runtime.error = Some(PLACEMENT.into());
    runtime.progress.finish(Instant::now());
    let status = of(&runtime, Occupancy::default(), now());
    let failure = status.failure.unwrap();
    assert_eq!(status.numbers, PLACEMENT);
    assert_eq!(failure.cause, None);
    assert_eq!(failure.copy_text(), PLACEMENT);
    assert_eq!(failure.remedy(), None);
}

#[test]
fn revisiting_a_step_invalidates_its_cached_diagnosis_without_new_output() {
    let mut runtime = Runtime {
        stage: Some(Stage::Validate),
        ..Runtime::default()
    };
    runtime.progress.stage(Stage::Validate, Instant::now());
    runtime.push_log("error: old validation diagnostic".into());
    runtime.error = Some("Validation failed".into());
    assert_eq!(
        of(&runtime, Occupancy::default(), now()).numbers,
        "error: old validation diagnostic"
    );
    runtime.progress.stage(Stage::Build, Instant::now());
    runtime.progress.stage(Stage::Validate, Instant::now());
    let failure = Failure::of(&runtime, "Validation failed");
    assert_eq!(failure.cause, None, "the previous visit's diagnostic is stale");
    runtime.push_log("fatal: current validation failure".into());
    assert_eq!(
        Failure::of(&runtime, "Validation failed").headline(),
        "fatal: current validation failure"
    );
}

#[test]
fn changing_the_failed_step_invalidates_the_cached_diagnosis() {
    let mut runtime = Runtime {
        stage: Some(Stage::Push),
        ..Runtime::default()
    };
    runtime.push_log("error from registry: denied".into());
    assert!(Failure::of(&runtime, "Operation failed").remedy().is_some());
    runtime.stage = Some(Stage::Provision);
    assert_eq!(Failure::of(&runtime, "Operation failed").cause, None);
}

#[test]
fn a_saved_snapshot_cannot_replace_the_failed_steps_output() {
    for (failed_step, saved, summary, output) in [
        (
            Stage::Provision,
            "Replace",
            PLACEMENT,
            "error: replacement server placement failed",
        ),
        (
            Stage::Readiness,
            "Ready",
            "Readiness check failed",
            "Permission denied (publickey).",
        ),
    ] {
        let mut runtime = Runtime::default();
        for stage in [Stage::Replace, failed_step] {
            runtime.stage = Some(stage);
            runtime.progress.stage(stage, Instant::now());
            runtime.push_log(
                if stage == failed_step {
                    output
                } else {
                    "error: recovered release warning"
                }
                .into(),
            );
        }
        runtime.adopt_snapshot(deployment(saved, &bound(), &running_worker()));
        runtime.rebuild = Some(Attempt::new(Kind::Rebuild));
        runtime.show_failure(summary.into(), Instant::now());
        // A Replace record can require reconciliation before this failure view is shown.
        let status = failed(&runtime, summary);
        assert_eq!(
            status.verb,
            if failed_step == Stage::Provision {
                "Provisioning failed"
            } else {
                "Readiness check failed"
            }
        );
        let marked = status.track.current.map(|index| status.track.stages[index]);
        assert_eq!(
            marked,
            Some(if failed_step == Stage::Provision {
                Stage::Replace
            } else {
                failed_step
            })
        );
        assert!(status.track.failed);
        let failure = status.failure.unwrap();
        assert_eq!(failure.headline(), output);
        assert_eq!(failure.copy_text(), format!("{output}\n{summary}"));
        if failed_step == Stage::Readiness {
            assert!(failure.meaning.unwrap().contains("SSH key"));
        }
    }
}

#[test]
fn a_connection_failure_after_ready_uses_output_after_the_completed_attempt() {
    let mut runtime = Runtime {
        stage: Some(Stage::Readiness),
        ..Runtime::default()
    };
    runtime.progress.stage(Stage::Readiness, Instant::now());
    runtime.push_log("error: recovered readiness warning".into());
    runtime.stage = Some(Stage::Ready);
    runtime.progress.stage(Stage::Ready, Instant::now());
    runtime.push_log("SSH connection failed: connection reset".into());
    runtime.show_failure("Connection failed".into(), Instant::now());
    assert_eq!(
        Failure::of(&runtime, "Connection failed").headline(),
        "SSH connection failed: connection reset"
    );
}
