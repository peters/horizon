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
        placement: horizon_core::cloud_panel::Placement::default(),
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
                    let cases = [
                        (Confirmation::Stop, false),
                        (Confirmation::Delete, false),
                        (Confirmation::None, true),
                    ];
                    for (id, (confirmation, deleting)) in cases.into_iter().enumerate() {
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
                                registry_generation: None,
                                worker: None,
                                sessions: Vec::new(),
                                source_ready: true,
                                ready_after_seconds: Some(420),
                                ready_history: horizon_core::cloud_runtime::state::ReadyHistory::Observed,
                                stop_requested: false,
                                browserstack_released: false,
                                browserstack_targets: std::collections::BTreeSet::new(),
                                image_replacement: None,
                                session_restart: None,
                                timeline: None,
                                last_self_stop: None,
                            }),
                            ..Default::default()
                        };
                        let _sender = deleting.then(|| {
                            let (sender, receiver) = std::sync::mpsc::channel();
                            runtime.receiver = Some(receiver);
                            runtime.stage = Some(Stage::DeleteWorker);
                            runtime.progress.stage(Stage::DeleteWorker, std::time::Instant::now());
                            runtime
                                .progress
                                .update(horizon_core::cloud_runtime::progress::Progress::activity(
                                    "Deleting the worker and confirming its removal ".repeat(40),
                                ));
                            runtime
                                .logs
                                .extend(std::iter::repeat_n("Detailed release output ".repeat(20), 40));
                            sender
                        });
                        let id = u32::try_from(id).unwrap();
                        let response = runtime_frame(ui, id, |ui| {
                            assert!(profile_details(ui, id, &launch, &runtime, &|_| None).is_none());
                            runtime_actions(ui, id, &mut runtime);
                        });
                        assert!((response.response.rect.width() - RUNTIME_WIDTH).abs() < 0.1);
                        assert!((response.response.rect.height() - RUNTIME_HEIGHT).abs() < 0.1);
                        rects.push(response.response.rect);
                    }
                },
            )
            .discard_textures();
        assert!(rects.windows(2).all(|pair| pair[0].bottom() <= pair[1].top()));
    }
}

mod deletion;
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
        placement: horizon_core::cloud_panel::Placement::default(),
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
        registry_generation: None,
        worker: None,
        sessions: Vec::new(),
        source_ready: false,
        ready_after_seconds: None,
        ready_history: horizon_core::cloud_runtime::state::ReadyHistory::Unobserved,
        stop_requested: false,
        browserstack_released: true,
        browserstack_targets: std::collections::BTreeSet::new(),
        image_replacement: None,
        session_restart: None,
        timeline: None,
        last_self_stop: None,
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
        placement: horizon_core::cloud_panel::Placement::default(),
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
            |ui| size = profile_details(ui, 1, launch, runtime, &|_| None),
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
                assert!(runtime_actions(ui, 1, &mut runtime).is_none());
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

/// 2024-07-12T19:14:40.144Z, the synthetic worker's latest start.
const RUN_STARTED_MS: u64 = 1_720_811_680_144;

fn cost_runtime(stage: Stage, worker: &serde_json::Value) -> super::super::Runtime {
    super::super::Runtime {
        stage: Some(stage),
        state: Some(
            serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":"cost-fixture","repository":"/synthetic","revision":"a",
                "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
                "stage":stage,"operation":{"state":"bound","worker_id":"worker1"},
                "spec":null,"worker":worker,"sessions":[]
            }))
            .unwrap(),
        ),
        ..Default::default()
    }
}

fn cost_worker(status: &str, fields: &serde_json::Value) -> serde_json::Value {
    let mut worker = serde_json::json!({
        "id":"worker1","name":"horizon-cloud-cost-fixture","imageName":"registry.example/worker",
        "desiredStatus":status,"publicIp":"192.0.2.1","portMappings":{"22":22001},
        "costPerHr":0.74,"adjustedCostPerHr":0.69,"lastStartedAt":"2024-07-12T19:14:40.144Z"
    });
    for (key, value) in fields.as_object().unwrap() {
        worker[key] = value.clone();
    }
    worker
}

fn cost_texts(runtime: &super::super::Runtime, elapsed: std::time::Duration) -> Vec<String> {
    let now = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(RUN_STARTED_MS) + elapsed;
    egui::Context::default()
        .run_ui(egui::RawInput::default(), |ui| worker_cost(ui, runtime, now))
        .discard_textures()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn ready_card_shows_the_live_cost_of_the_current_run() {
    let running = cost_runtime(Stage::Ready, &cost_worker("RUNNING", &serde_json::json!({})));
    let hour = std::time::Duration::from_hours(1);
    assert_eq!(
        cost_texts(&running, hour + std::time::Duration::from_mins(12)),
        ["This run · 1h 12m · $0.83 · $0.690/h"]
    );
    assert_eq!(
        cost_texts(&running, std::time::Duration::from_secs(307)),
        ["This run · 5m 07s · $0.06 · $0.690/h"]
    );
    let listed = cost_runtime(
        Stage::Ready,
        &cost_worker("RUNNING", &serde_json::json!({"adjustedCostPerHr": null})),
    );
    assert_eq!(cost_texts(&listed, hour), ["This run · 1h 00m · $0.74 · $0.740/h"]);
}

#[test]
fn card_falls_back_to_the_hourly_rate_without_a_live_run() {
    for runtime in [
        cost_runtime(Stage::Stopped, &cost_worker("EXITED", &serde_json::json!({}))),
        cost_runtime(Stage::Readiness, &cost_worker("RUNNING", &serde_json::json!({}))),
        cost_runtime(
            Stage::Ready,
            &cost_worker("RUNNING", &serde_json::json!({"lastStartedAt": null})),
        ),
    ] {
        assert_eq!(
            cost_texts(&runtime, std::time::Duration::from_secs(60)),
            ["Worker rate: $0.690/h"]
        );
    }
    let unpriced = cost_runtime(
        Stage::Ready,
        &cost_worker(
            "RUNNING",
            &serde_json::json!({"costPerHr": null, "adjustedCostPerHr": null}),
        ),
    );
    assert!(cost_texts(&unpriced, std::time::Duration::from_secs(60)).is_empty());
    assert!(cost_texts(&super::super::Runtime::default(), std::time::Duration::ZERO).is_empty());
}

/// The repaint delay a runtime requests after its first idle passes.
fn delay(runtime: &mut super::super::Runtime) -> std::time::Duration {
    let ctx = egui::Context::default();
    // A new context repaints its first passes immediately.
    for _ in 0..2 {
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {}).discard_textures();
    }
    ctx.run_ui(egui::RawInput::default(), |ui| {
        runtime.poll_release_and_repaint(ui.ctx());
    })
    .discard_textures()
    .viewport_output[&egui::ViewportId::ROOT]
        .repaint_delay
}

#[test]
fn only_a_ready_running_worker_schedules_the_one_second_refresh() {
    let mut running = cost_runtime(Stage::Ready, &cost_worker("RUNNING", &serde_json::json!({})));
    // egui subtracts the expected frame time from the requested delay.
    let refresh = delay(&mut running);
    assert!(refresh <= super::super::RUN_COST_REFRESH && refresh > super::super::RUN_COST_REFRESH / 2);
    for mut idle in [
        cost_runtime(Stage::Stopped, &cost_worker("EXITED", &serde_json::json!({}))),
        cost_runtime(
            Stage::Ready,
            &cost_worker("RUNNING", &serde_json::json!({"lastStartedAt": null})),
        ),
        super::super::Runtime::default(),
    ] {
        assert!(delay(&mut idle) > std::time::Duration::from_secs(60));
    }
}

/// Two completed charges and a partial latest hour that the run has been filling since 19:14:40.
fn billing() -> Vec<horizon_core::cloud_runtime::billing::BillingBucket> {
    use horizon_core::cloud_runtime::billing::{BillingBucket, BucketSize};
    [
        ("2024-07-11T00:00:00Z", BucketSize::Day, 2.0),
        ("2024-07-12T18:00:00Z", BucketSize::Hour, 0.69),
        ("2024-07-12T19:00:00Z", BucketSize::Hour, 0.1),
    ]
    .into_iter()
    .map(|(time, size, amount)| BillingBucket {
        time: time.into(),
        size,
        amount,
        time_billed_ms: 0,
    })
    .collect()
}

/// 2024-07-11T00:00:00Z, the fixture's first charge.
const FIRST_CHARGE: u64 = 1_720_656_000;

/// Billing read since `from` (Unix seconds).
fn billed_since(mut runtime: super::super::Runtime, from: u64) -> super::super::Runtime {
    let history = horizon_core::cloud_runtime::billing::History {
        buckets: billing(),
        from: std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(from),
    };
    runtime
        .billing
        .record("worker1", Ok(history), std::time::Instant::now());
    runtime
}

fn billed(runtime: super::super::Runtime) -> super::super::Runtime {
    billed_since(runtime, FIRST_CHARGE - 365 * 86_400)
}

fn badge(runtime: &super::super::Runtime, elapsed: std::time::Duration) -> Option<String> {
    let started = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(RUN_STARTED_MS);
    runtime.cost_badge(started + elapsed)
}

#[test]
fn card_and_header_add_the_total_since_creation_to_a_live_run() {
    let running = billed(cost_runtime(
        Stage::Ready,
        &cost_worker("RUNNING", &serde_json::json!({})),
    ));
    let elapsed = std::time::Duration::from_mins(72);
    assert_eq!(
        cost_texts(&running, elapsed),
        [
            "This run · 1h 12m · $0.83 · $0.690/h",
            "Since creation · $3.52 (billed $2.69 + $0.83 estimated)"
        ]
    );
    assert_eq!(badge(&running, elapsed).as_deref(), Some("$0.83 run · $3.52 total"));
    let reconnecting = billed(cost_runtime(
        Stage::Readiness,
        &cost_worker("RUNNING", &serde_json::json!({})),
    ));
    assert_eq!(badge(&reconnecting, elapsed).as_deref(), Some("$3.52 total"));
}

#[test]
fn a_stopped_cloud_keeps_showing_its_billed_total() {
    let stopped = billed(cost_runtime(
        Stage::Stopped,
        &cost_worker("EXITED", &serde_json::json!({})),
    ));
    let elapsed = std::time::Duration::from_hours(3);
    assert_eq!(
        cost_texts(&stopped, elapsed),
        [
            "Worker rate: $0.690/h",
            "Since creation · $2.79 (billed $2.79 + $0.00 estimated)"
        ]
    );
    assert_eq!(badge(&stopped, elapsed).as_deref(), Some("$2.79 total"));
    assert_eq!(
        badge(
            &cost_runtime(Stage::Stopped, &cost_worker("EXITED", &serde_json::json!({}))),
            elapsed
        ),
        None
    );
}

#[test]
fn a_worker_billed_before_the_read_window_shows_the_window_instead_of_a_lifetime() {
    let elapsed = std::time::Duration::from_mins(72);
    let stopped = billed_since(
        cost_runtime(Stage::Stopped, &cost_worker("EXITED", &serde_json::json!({}))),
        FIRST_CHARGE,
    );
    assert_eq!(
        cost_texts(&stopped, elapsed),
        [
            "Worker rate: $0.690/h",
            "Past 12 months · $2.79 (billed $2.79 + $0.00 estimated)"
        ]
    );
    assert_eq!(badge(&stopped, elapsed).as_deref(), Some("$2.79 12 mo"));
    let running = billed_since(
        cost_runtime(Stage::Ready, &cost_worker("RUNNING", &serde_json::json!({}))),
        FIRST_CHARGE,
    );
    assert_eq!(badge(&running, elapsed).as_deref(), Some("$0.83 run · $3.52 12 mo"));
}

#[test]
fn an_idle_followed_cloud_repaints_when_its_next_billing_refresh_is_due() {
    use horizon_core::cloud_runtime::billing::REFRESH_INTERVAL;
    let mut stopped = billed(cost_runtime(
        Stage::Stopped,
        &cost_worker("EXITED", &serde_json::json!({})),
    ));
    let due = delay(&mut stopped);
    assert!(due <= REFRESH_INTERVAL && due + std::time::Duration::from_secs(10) > REFRESH_INTERVAL);
    let mut running = billed(cost_runtime(
        Stage::Ready,
        &cost_worker("RUNNING", &serde_json::json!({})),
    ));
    assert!(
        delay(&mut running) <= super::super::RUN_COST_REFRESH,
        "the run meter's refresh still wins"
    );
    stopped.billing.stop();
    assert!(delay(&mut stopped) > REFRESH_INTERVAL, "an unfollowed cloud stays idle");
}

#[test]
fn billing_failures_hide_only_the_total_and_keep_the_run_meter() {
    let mut running = cost_runtime(Stage::Ready, &cost_worker("RUNNING", &serde_json::json!({})));
    let failure = Err(horizon_core::cloud_runtime::billing::BillingError::Unreachable);
    running
        .billing
        .record("worker1", failure.clone(), std::time::Instant::now());
    let elapsed = std::time::Duration::from_secs(307);
    assert_eq!(
        cost_texts(&running, elapsed),
        [
            "This run · 5m 07s · $0.06 · $0.690/h",
            "Total unavailable: RunPod is unreachable"
        ]
    );
    assert_eq!(badge(&running, elapsed).as_deref(), Some("$0.06 run"));
    let mut stale = billed(cost_runtime(
        Stage::Ready,
        &cost_worker("RUNNING", &serde_json::json!({})),
    ));
    stale.billing.record("worker1", failure, std::time::Instant::now());
    assert_eq!(
        cost_texts(&stale, std::time::Duration::from_mins(72))[1],
        "Since creation · $3.52 (billed $2.69 + $0.83 estimated)",
        "a failed refresh keeps the last billing"
    );
}

#[test]
fn the_first_refresh_shows_that_billing_is_being_read() {
    let stalled: horizon_core::cloud_runtime::billing::Fetch = |_, _, _, cancel| {
        while !cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Err(horizon_core::cloud_runtime::billing::BillingError::Unavailable)
    };
    let mut running = cost_runtime(Stage::Ready, &cost_worker("RUNNING", &serde_json::json!({})));
    running.billing.follow(
        running.state.as_ref(),
        Some(std::path::Path::new("/synthetic/cloud")),
        stalled,
        &|| {},
    );
    assert_eq!(
        cost_texts(&running, std::time::Duration::from_secs(60)),
        [
            "This run · 1m 00s · $0.01 · $0.690/h",
            "Since creation · reading RunPod billing…"
        ]
    );
    running.billing.stop();
    assert_eq!(cost_texts(&running, std::time::Duration::from_secs(60)).len(), 1);
}

#[test]
fn deleted_cloud_can_redeploy_without_removing_the_card() {
    let launch = size_launch();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    let mut runtime = super::super::Runtime {
        stage: Some(Stage::Deleted),
        state: Some(
            serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":"resize","repository":"/synthetic","revision":"a",
                "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":8,"memory_gb":32,"gpu":false},
                "stage":"Deleted","operation":{"state":"terminated","worker_id":"worker1"},
                "spec":null,"worker":null,"sessions":[]
            }))
            .unwrap(),
        ),
        ..Default::default()
    };
    let frame = |runtime: &mut super::super::Runtime, events: Vec<egui::Event>| {
        let mut action = None;
        let output = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(1200.0, 2000.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    assert!(profile_details(ui, 1, &launch, runtime, &|_| None).is_none());
                    action = runtime_actions(ui, 1, runtime);
                },
            )
            .discard_textures();
        let texts: Vec<(String, egui::Pos2)> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((text.galley.text().to_string(), text.pos + text.galley.size() * 0.5)),
                _ => None,
            })
            .collect();
        (texts, action)
    };
    let click = |runtime: &mut super::super::Runtime, label: &str| {
        let point = frame(runtime, Vec::new())
            .0
            .into_iter()
            .find_map(|(text, point)| (text == label).then_some(point))
            .unwrap_or_else(|| panic!("{label} must be rendered"));
        let mut action = None;
        for pressed in [true, false] {
            action = frame(
                runtime,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            )
            .1
            .or(action);
        }
        action
    };
    let (texts, action) = frame(&mut runtime, Vec::new());
    assert!(
        texts.iter().any(|(text, _)| text == "8 vCPU"),
        "a deleted cloud can choose the replacement size"
    );
    assert!(texts.iter().any(|(text, _)| text == "Redeploy cloud…"));
    assert!(texts.iter().any(|(text, _)| text == "Remove cloud"));
    assert!(action.is_none());
    assert!(click(&mut runtime, "Redeploy cloud…").is_none());
    assert!(runtime.confirmation == Confirmation::Redeploy);
    let (texts, _) = frame(&mut runtime, Vec::new());
    assert!(texts.iter().any(|(text, _)| text == "Redeploy cloud"));
    assert!(texts.iter().any(|(text, _)| text == "Keep removed"));
    assert!(click(&mut runtime, "Keep removed").is_none());
    assert!(runtime.confirmation == Confirmation::None);
    assert!(click(&mut runtime, "Redeploy cloud…").is_none());
    assert!(click(&mut runtime, "Redeploy cloud") == Some(Action::Deploy));
    runtime.confirmation = Confirmation::None;
    assert!(click(&mut runtime, "Remove cloud") == Some(Action::Remove));
}

#[test]
fn an_active_redeploy_keeps_the_selected_size_and_status() {
    let mut launch = size_launch();
    launch.profile.cpu = 16;
    let ctx = egui::Context::default();
    let (_sender, receiver) = std::sync::mpsc::channel();
    let mut runtime = super::super::Runtime {
        stage: Some(Stage::Validate),
        receiver: Some(receiver),
        state: Some(
            serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":"resize","repository":"/synthetic","revision":"a",
                "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":8,"memory_gb":32,"gpu":false},
                "stage":"Deleted","operation":{"state":"terminated","worker_id":"worker1"},
                "spec":null,"worker":null,"sessions":[]
            }))
            .unwrap(),
        ),
        ..Default::default()
    };
    let output = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            assert!(profile_details(ui, 1, &launch, &runtime, &|_| None).is_none());
            assert!(runtime_actions(ui, 1, &mut runtime).is_none());
        })
        .discard_textures();
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|text| text == "16 vCPU · 32 GB · CPU only"));
    assert!(texts.iter().any(|text| text == "Redeploying cloud…"));
    assert!(
        texts
            .iter()
            .all(|text| !text.contains("Finish managed workspace storage cleanup"))
    );
}
