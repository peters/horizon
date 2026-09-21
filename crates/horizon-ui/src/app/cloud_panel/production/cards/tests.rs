use super::*;
use crate::test_egui::DiscardTextures;
use horizon_core::{cloud_panel::CloudLaunch, cloud_runtime::state::Deployment};

#[test]
fn runtime_cards_keep_reserved_bounds_with_long_details_and_confirmations() {
    let mut config = super::super::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/team/worker\n    cpu: 4\n    memory_gb: 8\n",
    )
    .unwrap();
    let launch = CloudLaunch {
        deployment_started: true,
        id: "runtime-bounds".into(),
        revision: "a".repeat(40),
        profile_name: "Long development profile ".repeat(8),
        profile: config.profiles.remove("dev").unwrap(),
    };
    let ctx = egui::Context::default();
    for _ in 0..3 {
        let mut rects = Vec::new();
        let _ = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 2000.0))),
                    ..Default::default()
                },
                |ui| {
                    for (id, confirmation) in [Confirmation::Stop, Confirmation::Delete].into_iter().enumerate() {
                        let mut runtime = super::super::Runtime {
                            stage: Some(Stage::Ready),
                            error: Some("Detailed retryable failure ".repeat(200)),
                            confirmation,
                            state: Some(Deployment {
                                version: 1,
                                cloud_id: launch.id.clone(),
                                repository: "/synthetic".into(),
                                revision: launch.revision.clone(),
                                profile: launch.profile.clone(),
                                stage: Stage::Ready,
                                operation: horizon_core::cloud_runtime::CreateState::Bound {
                                    worker_id: "worker1".into(),
                                },
                                spec: None,
                                worker: None,
                                sessions: Vec::new(),
                                source_ready: true,
                                ready_after_seconds: Some(420),
                                ready_history: horizon_core::cloud_runtime::state::ReadyHistory::Observed,
                                stop_requested: false,
                                browserstack_released: false,
                                browserstack_targets: std::collections::BTreeSet::new(),
                            }),
                            ..Default::default()
                        };
                        let response = runtime_frame(ui, u32::try_from(id).unwrap(), |ui| {
                            profile_details(ui, &launch);
                            runtime_actions(ui, &mut runtime);
                        });
                        assert!((response.response.rect.width() - RUNTIME_WIDTH).abs() < 0.1);
                        assert!((response.response.rect.height() - RUNTIME_HEIGHT).abs() < 0.1);
                        rects.push(response.response.rect);
                    }
                },
            )
            .discard_textures();
        assert!(rects[0].bottom() <= rects[1].top());
    }
}

mod scrolling;

#[test]
fn recovery_result_keeps_unknown_requests_fenced_without_auto_attach() {
    use horizon_core::cloud_runtime::{self, lifecycle::ReconciledDeployment};
    let mut runtime = super::super::Runtime::default();
    let (tx, rx) = std::sync::mpsc::channel();
    runtime.recovery_receiver = Some(rx);
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"recovery-fixture","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Provision","operation":{"state":"requested"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    tx.send(Ok(ReconciledDeployment {
        state,
        report: serde_json::from_value(
            serde_json::json!({"operation_id":"recovery-fixture","outcome":{"status":"unresolved"}}),
        )
        .unwrap(),
    }))
    .unwrap();
    runtime.poll_recovery();
    assert!(runtime.recovery_receiver.is_none());
    assert_eq!(
        runtime.state.as_ref().unwrap().operation,
        cloud_runtime::CreateState::Requested
    );
    assert!(runtime.error.as_ref().unwrap().contains("does not prove failure"));
    assert!(!runtime.needs_attach);
    assert!(runtime.receiver.is_none());
}

#[test]
fn recovery_transport_failure_preserves_state_and_allows_another_check() {
    let mut runtime = super::super::Runtime::default();
    let (tx, rx) = std::sync::mpsc::channel();
    runtime.recovery_receiver = Some(rx);
    tx.send(Err(horizon_core::cloud_runtime::Error::Busy)).unwrap();
    runtime.poll_recovery();
    assert!(runtime.recovery_receiver.is_none());
    assert!(runtime.error.as_ref().unwrap().contains("Another controller"));
    assert!(!runtime.needs_attach);
}
