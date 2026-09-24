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
                        let id = u32::try_from(id).unwrap();
                        let response = runtime_frame(ui, id, |ui| {
                            assert!(profile_details(ui, id, &launch, &runtime).is_none());
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

mod overlap;
mod scrolling;

#[test]
#[cfg(unix)]
fn resizing_requires_a_saved_record_without_a_requested_worker() {
    let (temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = horizon_core::cloud_panel::CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    let config = super::super::CloudConfig::parse("version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example/worker:latest\n    cpu: 8\n    memory_gb: 32\n").unwrap();
    let profile = config.profiles["dev"].clone();
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: profile.clone(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(temp.path().into());
    let size = |app: &crate::app::HorizonApp| {
        let profile = &app.cloud_prototype.groups.0[0].remote.as_ref().unwrap().profile;
        (profile.cpu, profile.memory_gb)
    };
    app.resize_production_cloud(1, (16, 64));
    assert_eq!(size(&app), (8, 32), "a missing deployed record keeps the size");
    let root = temp.path().join("fixture");
    let store = super::super::Store::lock(&root).unwrap();
    let mut state = horizon_core::cloud_runtime::state::Deployment {
        version: 1,
        cloud_id: "fixture".into(),
        repository: temp.path().into(),
        revision: "a".repeat(40),
        profile,
        stage: Stage::Provision,
        operation: horizon_core::cloud_runtime::CreateState::Requested,
        spec: None,
        worker: None,
        sessions: Vec::new(),
        source_ready: false,
        ready_after_seconds: None,
        ready_history: horizon_core::cloud_runtime::state::ReadyHistory::Unobserved,
        stop_requested: false,
        browserstack_released: true,
        browserstack_targets: std::collections::BTreeSet::new(),
    };
    store.save(&state).unwrap();
    app.resize_production_cloud(1, (16, 64));
    assert_eq!(size(&app), (8, 32), "a competing controller keeps the size");
    drop(store);
    app.resize_production_cloud(1, (16, 64));
    assert_eq!(size(&app), (8, 32), "a requested worker keeps the size");
    assert_eq!(
        app.cloud_prototype.production.runtimes[&1]
            .state
            .as_ref()
            .unwrap()
            .operation,
        horizon_core::cloud_runtime::CreateState::Requested,
        "the saved record replaces a stale snapshot"
    );
    state.operation = horizon_core::cloud_runtime::CreateState::Prepared;
    super::super::Store::lock(&root).unwrap().save(&state).unwrap();
    app.resize_production_cloud(1, (16, 64));
    assert_eq!(size(&app), (16, 64));
}

fn size_launch() -> CloudLaunch {
    let mut config = super::super::CloudConfig::parse(
        "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/team/worker\n    cpu: 8\n    memory_gb: 32\n",
    )
    .unwrap();
    CloudLaunch {
        deployment_started: true,
        id: "resize".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: config.profiles.remove("dev").unwrap(),
    }
}

/// Visible texts with their centers, and any size chosen in that frame.
type SizeFrame = (Vec<(String, Pos2)>, Option<(u16, u16)>);

/// Renders the profile details once.
fn size_frame(
    ctx: &egui::Context,
    launch: &CloudLaunch,
    runtime: &super::super::Runtime,
    events: Vec<egui::Event>,
) -> SizeFrame {
    let mut size = None;
    let output = ctx
        .run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| size = profile_details(ui, 1, launch, runtime),
        )
        .discard_textures();
    let texts = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos + text.galley.size() * 0.5)),
            _ => None,
        })
        .collect();
    (texts, size)
}

#[test]
fn machine_size_offers_only_runpod_sizes_before_a_worker_is_requested() {
    let launch = size_launch();
    let ctx = egui::Context::default();
    // The app theme pads drop-downs above the default row height.
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    let runtime = super::super::Runtime::default();
    let frame = |events| size_frame(&ctx, &launch, &runtime, events);
    let click = |label: &str| {
        let point = frame(Vec::new())
            .0
            .into_iter()
            .find_map(|(text, point)| (text == label).then_some(point))
            .unwrap_or_else(|| panic!("{label} must be rendered"));
        let mut size = None;
        for pressed in [true, false] {
            let events = vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            size = frame(events).1.or(size);
        }
        size
    };
    let row = frame(Vec::new()).0;
    let height = |label: &str| row.iter().find_map(|(text, point)| (text == label).then_some(point.y));
    assert!(height("8 vCPU").is_some());
    assert_eq!(height("8 vCPU"), height("32 GB"), "drop-downs share one row");
    assert!(click("32 GB").is_none());
    assert_eq!(click("64 GB · memory-optimized"), Some((8, 64)));
    assert!(click("8 vCPU").is_none());
    let offered = frame(Vec::new()).0;
    assert!(offered.iter().any(|(text, _)| text == "32 vCPU"));
    assert!(
        !offered.iter().any(|(text, _)| text == "1 vCPU"),
        "RunPod CPU pods need a power of two from 2 vCPU"
    );
    assert_eq!(click("16 vCPU"), Some((16, 64)), "vCPU changes keep the memory family");
}

#[test]
fn machine_size_is_fixed_while_busy_or_once_a_worker_is_requested() {
    let launch = size_launch();
    let ctx = egui::Context::default();
    let mut runtime = super::super::Runtime::default();
    let locked = |runtime: &super::super::Runtime, label: &str| {
        let (texts, size) = size_frame(&ctx, &launch, runtime, Vec::new());
        assert!(size.is_none());
        assert!(texts.iter().any(|(text, _)| text == label), "{label} must be shown");
        assert!(!texts.iter().any(|(text, _)| text == "8 vCPU"), "no size choice");
    };
    runtime.state_unavailable = true;
    locked(&runtime, "8 vCPU · 32 GB · CPU only");
    runtime.state_unavailable = false;
    let (_sender, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    locked(&runtime, "8 vCPU · 32 GB · CPU only");
    runtime.receiver = None;
    // A requested worker shows its saved size, even if the launch profile differs.
    for (operation, worker) in [
        (serde_json::json!({"state":"requested"}), serde_json::Value::Null),
        (
            serde_json::json!({"state":"bound","worker_id":"worker1"}),
            serde_json::json!({"id":"worker1","name":"resize","imageName":"example.invalid/team/worker","desiredStatus":"RUNNING"}),
        ),
    ] {
        runtime.state = Some(
            serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":"resize","repository":"/synthetic","revision":"a",
                "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":4,"memory_gb":16,"gpu":false},
                "stage":"Provision","operation":operation,"spec":null,"worker":worker,"sessions":[]
            }))
            .unwrap(),
        );
        locked(&runtime, "4 vCPU · 16 GB · CPU only");
    }
}

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

#[test]
fn inactive_recovery_retains_deletion_without_automatic_reconnect() {
    use horizon_core::cloud_runtime::{self, lifecycle::ReconciledDeployment};
    for status in ["EXITED", "TERMINATED", "UNKNOWN"] {
        let mut runtime = super::super::Runtime {
            recovery_worker_id: "unlisted-old-hint".into(),
            ..Default::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        runtime.recovery_receiver = Some(rx);
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"recovery-fixture","repository":"/synthetic","revision":"a",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
            "stage":"Provision","operation":{"state":"bound","worker_id":"worker1"},"spec":null,
            "worker":{"id":"worker1","name":"recovery-fixture","imageName":"registry.example/worker","desiredStatus":status},
            "sessions":[]
        })).unwrap();
        tx.send(Ok(ReconciledDeployment {
            state,
            report: serde_json::from_value(serde_json::json!({
                "operation_id":"recovery-fixture","outcome":{"status":"inactive","worker_id":"worker1"}
            }))
            .unwrap(),
        }))
        .unwrap();
        runtime.poll_recovery();
        assert!(runtime.recovery_worker_id.is_empty());
        let state = runtime.state.as_ref().unwrap();
        assert!(super::super::Runtime::needs_provider_check(state));
        assert!(matches!(state.operation, cloud_runtime::CreateState::Bound { .. }));
        assert_eq!(runtime.stage, Some(Stage::Provision));
        assert!(runtime.error.as_ref().unwrap().contains("cleanup is not confirmed"));
        assert!(runtime.receiver.is_none());
        assert!(!runtime.needs_attach);
    }
}

#[test]
fn restored_bound_records_can_check_provider_after_failed_reconnect() {
    for worker in [
        serde_json::Value::Null,
        serde_json::json!({
            "id":"worker1","name":"recovery-fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING"
        }),
    ] {
        let saved = serde_json::json!({
            "version":1,"cloud_id":"recovery-fixture","repository":"/synthetic","revision":"a",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
            "stage":"Provision","operation":{"state":"bound","worker_id":"worker1"},
            "spec":null,"worker":worker,"sessions":[]
        });
        let mut runtime = super::super::Runtime {
            state: Some(serde_json::from_str(&saved.to_string()).unwrap()),
            error: Some("Existing worker is not running; check provider before reconnecting".into()),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut point = Pos2::ZERO;
        for _ in 0..2 {
            let output = ctx
                .run_ui(egui::RawInput::default(), |ui| {
                    assert!(bound_provider_check(ui, &runtime).is_none());
                })
                .discard_textures();
            point = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Check provider" => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("Bound recovery action must be rendered for stale and absent metadata");
        }
        let mut action = None;
        for pressed in [true, false] {
            let _ = ctx
                .run_ui(
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(point),
                            egui::Event::PointerButton {
                                pos: point,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        action = bound_provider_check(ui, &runtime).or(action);
                    },
                )
                .discard_textures();
        }
        assert!(matches!(action, Some(Action::Reconcile)));
        let (_tx, rx) = std::sync::mpsc::channel();
        runtime.recovery_receiver = Some(rx);
        let _ = ctx
            .run_ui(egui::RawInput::default(), |ui| {
                assert!(runtime_actions(ui, &mut runtime).is_none());
            })
            .discard_textures();
    }
}

#[test]
fn missing_worker_recovery_keeps_the_warning_despite_cached_running_status() {
    use horizon_core::cloud_runtime::{self, lifecycle::ReconciledDeployment};
    for worker in [
        serde_json::Value::Null,
        serde_json::json!({
            "id":"worker1","name":"recovery-fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING"
        }),
    ] {
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"recovery-fixture","repository":"/synthetic","revision":"a",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
            "spec":null,"worker":worker,"sessions":[]
        }))
        .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut runtime = super::super::Runtime {
            recovery_receiver: Some(rx),
            ..Default::default()
        };
        tx.send(Ok(ReconciledDeployment {
            state,
            report: serde_json::from_value(serde_json::json!({
                "operation_id":"recovery-fixture","outcome":{"status":"missing","worker_id":"worker1"}
            }))
            .unwrap(),
        }))
        .unwrap();
        runtime.poll_recovery();
        assert!(
            runtime
                .error
                .as_ref()
                .unwrap()
                .contains("no longer returned by the provider")
        );
        assert_eq!(
            runtime.state.as_ref().unwrap().operation,
            cloud_runtime::CreateState::Bound {
                worker_id: "worker1".into()
            }
        );
        assert!(!runtime.needs_attach);
        assert!(runtime.receiver.is_none());
    }
}
