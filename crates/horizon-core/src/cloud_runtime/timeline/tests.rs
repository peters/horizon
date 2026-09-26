use super::*;
use crate::cloud_runtime::progress::Progress;

/// Tenths of a second after a fixed origin, so expected milliseconds are exact.
fn at(tenths: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_000) + Duration::from_millis(tenths * 100)
}

fn stage(recorder: &Recorder, stage: Stage, tenths: u64) {
    recorder.observe_at(&Event::stage(stage), at(tenths));
}

fn activity(recorder: &Recorder, detail: &str, tenths: u64) {
    recorder.observe_at(&Event::Progress(Progress::activity(detail)), at(tenths));
}

fn millis(timeline: &Timeline, phase: Phase) -> u64 {
    timeline
        .spans
        .iter()
        .filter(|span| span.phase == phase)
        .map(|span| span.millis)
        .sum()
}

/// The measured 2026-09-26 deployment: 148 s of image download inside readiness.
fn fresh_deployment() -> Recorder {
    let recorder = Recorder::default();
    stage(&recorder, Stage::Validate, 0);
    stage(&recorder, Stage::Build, 98);
    stage(&recorder, Stage::Push, 119);
    stage(&recorder, Stage::Provision, 153);
    stage(&recorder, Stage::Readiness, 219);
    activity(&recorder, AWAITING_ENDPOINT, 229);
    activity(&recorder, AWAITING_SERVICES, 1864);
    activity(&recorder, AWAITING_SERVICES, 1869);
    stage(&recorder, Stage::Worktrees, 1877);
    activity(&recorder, UPLOADING_SOURCE, 1877);
    activity(&recorder, IMPORTING_OBJECTS, 2020);
    activity(&recorder, UPLOADING_SOURCE, 2330);
    activity(&recorder, IMPORTING_DEPENDENCIES, 2354);
    stage(&recorder, Stage::Sessions, 2551);
    recorder
}

#[test]
fn the_container_start_separates_the_image_download_from_boot_and_publication() {
    let timeline = fresh_deployment().finish(false, Some(at(1698)), at(2567), None);
    assert_eq!(timeline.label(Phase::ProviderStart), "Image download");
    assert_eq!(timeline.total(), Duration::from_millis(256_700));
    assert_eq!(millis(&timeline, Phase::Prepare), 9_800);
    assert_eq!(millis(&timeline, Phase::ProviderStart), 147_900);
    assert_eq!(millis(&timeline, Phase::WorkerStart), 16_600);
    assert_eq!(millis(&timeline, Phase::Readiness), 1_300);
    assert_eq!(millis(&timeline, Phase::SourceUpload), 16_700);
    assert_eq!(millis(&timeline, Phase::SourceImport), 50_700);
    assert_eq!(millis(&timeline, Phase::Sessions), 1_600);
    let phases = timeline.phases();
    assert_eq!(phases[0], (Phase::ProviderStart, Duration::from_millis(147_900)));
    assert_eq!(phases[1].0, Phase::SourceImport);
    assert_eq!(
        phases.iter().map(|(_, duration)| *duration).sum::<Duration>(),
        timeline.total()
    );
}

#[test]
fn older_workers_keep_boot_inside_the_provider_phase() {
    let timeline = fresh_deployment().finish(false, None, at(2567), None);
    assert_eq!(millis(&timeline, Phase::ProviderStart), 164_500);
    assert_eq!(millis(&timeline, Phase::WorkerStart), 0);
    assert!(timeline.spans.iter().all(|span| span.phase != Phase::WorkerStart));
    assert_eq!(millis(&timeline, Phase::Readiness), 1_300);
}

#[test]
fn a_skewed_or_early_container_start_is_clamped_into_the_readiness_window() {
    for (reported, provider) in [(100, 0), (4_000, 165_800), (1_000, 78_100)] {
        let timeline = fresh_deployment().finish(false, Some(at(reported)), at(2567), None);
        assert_eq!(millis(&timeline, Phase::ProviderStart), provider, "{reported}");
        assert_eq!(timeline.total(), Duration::from_millis(256_700));
    }
}

#[test]
fn a_reconnection_with_a_known_endpoint_counts_boot_as_readiness() {
    let recorder = Recorder::default();
    stage(&recorder, Stage::Validate, 0);
    stage(&recorder, Stage::Provision, 1);
    stage(&recorder, Stage::Readiness, 12);
    activity(&recorder, AWAITING_SERVICES, 25);
    stage(&recorder, Stage::Sessions, 284);
    let timeline = recorder.finish(true, Some(at(60)), at(322), None);
    assert!(timeline.reconnected);
    assert_eq!(timeline.label(Phase::ProviderStart), "Worker start");
    assert_eq!(timeline.label(Phase::Sessions), Phase::Sessions.label());
    assert_eq!(millis(&timeline, Phase::ProviderStart), 4_800);
    assert_eq!(millis(&timeline, Phase::WorkerStart), 0);
    assert_eq!(millis(&timeline, Phase::Readiness), 22_400);
    assert_eq!(timeline.total(), Duration::from_millis(32_200));
}

#[test]
fn saved_timelines_round_trip_and_unrelated_events_are_ignored() {
    let recorder = fresh_deployment();
    recorder.observe_at(&Event::Output("hint: branch".into()), at(2500));
    activity(&recorder, "Removed agent credential bindings", 2555);
    let timeline = recorder.finish(false, Some(at(1698)), at(2567), None);
    let json = serde_json::to_value(&timeline).unwrap();
    assert_eq!(
        json["spans"][0],
        serde_json::json!({"phase": "prepare", "millis": 9800})
    );
    assert_eq!(serde_json::from_value::<Timeline>(json).unwrap(), timeline);
    assert_eq!(
        serde_json::from_str::<Timeline>(r#"{"spans":[]}"#).unwrap(),
        Timeline::default()
    );
}

#[test]
fn a_recent_resume_request_leads_the_reconnection_and_an_old_one_is_ignored() {
    let reconnection = || {
        let recorder = Recorder::default();
        stage(&recorder, Stage::Validate, 30);
        stage(&recorder, Stage::Readiness, 40);
        activity(&recorder, AWAITING_SERVICES, 50);
        stage(&recorder, Stage::Sessions, 300);
        recorder
    };
    let recent = Timeline::resume_requested(at(3));
    let timeline = reconnection().finish(true, None, at(320), Some(&recent));
    assert_eq!(
        timeline.spans[0],
        Span {
            phase: Phase::Provision,
            millis: 2_700
        }
    );
    assert_eq!(timeline.total(), Duration::from_millis(31_700));
    assert_eq!(timeline.resume_requested, None);
    let old = Timeline::resume_requested(at(0) - Duration::from_secs(3_600));
    let timeline = reconnection().finish(true, None, at(320), Some(&old));
    assert_eq!(timeline.total(), Duration::from_secs(29));
    // A pending resume shows nothing until its reconnection completes it.
    assert!(recent.total().is_zero());
}

#[test]
fn only_a_worker_that_was_ready_before_counts_as_a_reconnection() {
    let state = |operation: serde_json::Value, history: &str| -> crate::cloud_runtime::state::Deployment {
        let profile = serde_json::json!({"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8});
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"timeline","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":"Readiness","operation":operation,"spec":null,"worker":null,
            "sessions":[],"ready_history":history
        }))
        .unwrap()
    };
    let bound = serde_json::json!({"state":"bound","worker_id":"worker1"});
    assert!(reconnects(&state(bound.clone(), "Observed")));
    assert!(!reconnects(&state(bound, "Unobserved")));
    assert!(!reconnects(&state(serde_json::json!({"state":"prepared"}), "Observed")));
}
