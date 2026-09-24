use super::*;
use crate::app::test_support::test_app;
use horizon_core::{
    cloud_panel::{CloudConfig, CloudGroup, CloudLaunch},
    cloud_runtime::state::{Deployment, OperationId, ReplacementImage, ReplacementPhase, Store},
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const CONFIG: &str = "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: registry.example/worker\n    cpu: 4\n    memory_gb: 8\n    build:\n      context: .horizon\n      dockerfile: .horizon/Dockerfile\n";

/// The replacement journal a fixture records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::cloud_panel::production) enum Phase {
    None,
    Prepared,
    Built,
    Requested,
}

/// A Ready dedicated cloud on a running bound worker, with a build recipe when
/// `build`, and the journal `phase`.
pub(in crate::app::cloud_panel::production) fn deployment(repository: &Path, build: bool, phase: Phase) -> Deployment {
    let mut profile = CloudConfig::parse(CONFIG).unwrap().profiles.remove("dev").unwrap();
    if !build {
        profile.build = None;
    }
    let mut state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"fixture","repository":repository,"revision":"a".repeat(40),
        "profile":profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
        "spec":{
            "operation_id":"fixture","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "profile":profile,"public_key":"unused-fixture-key","registry_auth_id":null,"gpu_types":[],
            "cpu_flavors":["cpu3c"],"data_centers":[]
        },
        "worker":{"id":"worker1","name":"fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING"},
        "sessions":[],"source_ready":true
    }))
    .unwrap();
    if phase == Phase::None {
        return state;
    }
    state
        .begin_replacement(OperationId::generate(), "c".repeat(40))
        .unwrap();
    if phase == Phase::Prepared {
        return state;
    }
    state
        .replacement_built(ReplacementImage {
            digest: format!("registry.example/worker@sha256:{}", "b".repeat(64)),
            registry_auth_id: None,
            registry_generation: None,
        })
        .unwrap();
    if phase == Phase::Requested {
        state.request_replacement().unwrap();
    }
    state
}

#[test]
fn a_restored_requested_switch_waits_for_a_provider_check() {
    for phase in [Phase::None, Phase::Prepared, Phase::Built] {
        let state = deployment(Path::new("/synthetic"), true, phase);
        assert!(!Runtime::needs_provider_check(&state), "{phase:?}");
        assert!(
            Runtime::reconnects_on_restore(&state),
            "{phase:?}: the worker still runs its recorded image"
        );
    }
    let requested = deployment(Path::new("/synthetic"), true, Phase::Requested);
    assert_eq!(requested.stage, Stage::Replace);
    assert!(Runtime::needs_provider_check(&requested));
    assert!(
        !Runtime::reconnects_on_restore(&requested),
        "the worker may run either image"
    );
    let mut unjournaled = deployment(Path::new("/synthetic"), true, Phase::None);
    unjournaled.stage = Stage::Replace;
    assert!(Runtime::needs_provider_check(&unjournaled));
}

#[test]
fn only_the_replacement_outcomes_become_notes() {
    let mut runtime = Runtime::default();
    runtime.observe_rebuild(&Event::Output("Image unchanged; nothing to restart".into()));
    assert!(runtime.rebuild.is_none(), "outside a rebuild nothing is kept");
    runtime.rebuild = Some(Attempt::new(Kind::Rebuild));
    for line in [
        "#12 [4/9] RUN install-agents",
        "Agent CLI releases: claude 2.0.1, codex 0.9.0",
        "Session panel-1 (claude) lost its process in the container reset and was not relaunched",
        "Image unchanged; nothing to restart",
        "Image replacement cancelled; the worker keeps its image",
        "Transport error; checking which image the provider records",
    ] {
        runtime.observe_rebuild(&Event::Output(line.into()));
    }
    runtime.observe_rebuild(&Event::stage(Stage::Build));
    let notes: Vec<_> = runtime
        .rebuild
        .as_ref()
        .unwrap()
        .notes
        .iter()
        .map(|note| (note.text.as_str(), note.warning))
        .collect();
    assert_eq!(
        notes,
        [
            ("Agent CLI releases: claude 2.0.1, codex 0.9.0", false),
            (
                "Session panel-1 (claude) lost its process in the container reset and was not relaunched",
                true
            ),
            ("Image unchanged; nothing to restart", false),
            ("Image replacement cancelled; the worker keeps its image", false),
        ]
    );
    for _ in 0..NOTE_LIMIT {
        runtime.observe_rebuild(&Event::Output("Image unchanged; nothing to restart".into()));
    }
    assert_eq!(runtime.rebuild.as_ref().unwrap().notes.len(), NOTE_LIMIT);
}

/// A Ready cloud with a built, unsent replacement and settings without a provider
/// credential, so every core call fails before provider I/O.
fn seed_built_cloud(app: &mut HorizonApp, root: &Path) -> std::path::PathBuf {
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        root.into(),
        [0.0, 0.0],
    );
    let state = deployment(root, true, Phase::Built);
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: state.profile.clone(),
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(root.into());
    let settings = serde_json::json!({
        "runpod_key_file": root.join("missing-key"), "ssh_identity_file": root.join("ssh"),
        "docker_config": root.join("docker"), "registry_pull_auth_id": null,
        "cpu_flavors": [], "gpu_types": []
    });
    std::fs::write(root.join("settings.json"), settings.to_string()).unwrap();
    let state_root = root.join("fixture");
    Store::lock(&state_root).unwrap().save(&state).unwrap();
    state_root
}

/// Starts `action` over a presented Ready cloud and waits for its result.
fn run_action<'a>(app: &'a mut HorizonApp, ctx: &egui::Context, action: Action) -> &'a Runtime {
    let watch = cloud_runtime::Cancellation::default();
    let (_watch_sender, watch_receiver) = channel();
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.stage = Some(Stage::Ready);
    runtime.receiver = Some(watch_receiver);
    runtime.cancel = Some(watch.clone());
    runtime.confirmation = Confirmation::Rebuild;
    runtime.error = Some("Earlier failure".into());
    app.change_production_worker(1, action, ctx);
    assert!(watch.is_cancelled(), "the presentation watch stops first");
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert_eq!(runtime.rebuild.as_ref().map(|attempt| attempt.kind), Kind::of(action));
    assert_eq!(runtime.stage, Some(Stage::Validate));
    assert!(runtime.confirmation == Confirmation::None);
    assert!(runtime.error.is_none());
    assert!(runtime.desktop.is_none());
    assert!(runtime.busy());
    let kind = Kind::of(action);
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.cloud_prototype.production.runtimes[&1].receiver.is_some() {
        assert!(Instant::now() < deadline, "{kind:?} must finish");
        std::thread::sleep(Duration::from_millis(10));
        app.prepare_production_clouds(ctx);
    }
    &app.cloud_prototype.production.runtimes[&1]
}

fn saved_phase(state_root: &Path) -> Option<ReplacementPhase> {
    Store::lock(state_root)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .image_replacement
        .map(|journal| journal.phase)
}

#[test]
#[cfg_attr(
    windows,
    ignore = "cloud control requires durable directory updates, which are Unix-only"
)]
fn rebuild_actions_start_their_own_core_operation() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    let state_root = seed_built_cloud(&mut app, temp.path());

    // A new rebuild refuses the pending one without touching it.
    let runtime = run_action(&mut app, &ctx, Action::Rebuild);
    assert_eq!(
        runtime.error.as_deref(),
        Some("Image replacement pending; continue or cancel it")
    );
    assert!(matches!(
        runtime
            .state
            .as_ref()
            .unwrap()
            .image_replacement
            .as_ref()
            .unwrap()
            .phase,
        ReplacementPhase::Built(_)
    ));
    assert!(matches!(saved_phase(&state_root), Some(ReplacementPhase::Built(_))));

    // Continuing reaches for the provider credential and keeps the journal.
    let runtime = run_action(&mut app, &ctx, Action::ContinueRebuild);
    let error = runtime.error.clone().unwrap();
    assert!(!error.contains("pending"), "{error}");
    assert_eq!(runtime.stage, Some(Stage::Ready));
    assert!(matches!(saved_phase(&state_root), Some(ReplacementPhase::Built(_))));

    // Cancelling an unsent switch drops it locally, then reconnects.
    let runtime = run_action(&mut app, &ctx, Action::CancelRebuild);
    assert!(saved_phase(&state_root).is_none());
    assert!(runtime.state.as_ref().unwrap().image_replacement.is_none());
    let notes: Vec<_> = runtime
        .rebuild
        .as_ref()
        .unwrap()
        .notes
        .iter()
        .map(|note| note.text.as_str())
        .collect();
    assert_eq!(notes, ["Image replacement cancelled; the worker keeps its image"]);
    assert!(
        runtime.progress.stage_label(Stage::Validate).contains(" · "),
        "the reconnect ran"
    );
    assert!(runtime.error.is_some(), "the reconnect needs the provider credential");
    assert!(!runtime.needs_repaint());

    // Any other worker operation replaces the finished rebuild's view.
    for stop in [false, true] {
        let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
        runtime.stage = Some(Stage::Ready);
        runtime.rebuild = Some(Attempt::new(Kind::Rebuild));
        if stop {
            app.change_production_worker(1, Action::Stop, &ctx);
        } else {
            app.start_production_deployment(1, &ctx);
        }
        let runtime = &app.cloud_prototype.production.runtimes[&1];
        assert!(runtime.rebuild.is_none(), "stop: {stop}");
        assert!(runtime.receiver.is_some());
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.cloud_prototype.production.runtimes[&1].receiver.is_some() {
            assert!(Instant::now() < deadline, "the operation must finish");
            std::thread::sleep(Duration::from_millis(10));
            app.prepare_production_clouds(&ctx);
        }
    }
}

#[test]
fn a_busy_cloud_does_not_start_a_rebuild() {
    let (temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.prepare_production_clouds(&ctx);
    let workspace = app.board.create_workspace("cloud fixture");
    let mut group = CloudGroup::new(
        1,
        "Fixture".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().into(),
        [0.0, 0.0],
    );
    group.remote = Some(CloudLaunch {
        deployment_started: true,
        id: "fixture".into(),
        revision: "a".repeat(40),
        profile_name: "dev".into(),
        profile: deployment(temp.path(), true, Phase::None).profile,
    });
    app.cloud_prototype.groups.0.push(group);
    app.cloud_prototype.root = Some(temp.path().into());
    let (_sender, receiver) = channel();
    let runtime = app.cloud_prototype.production.runtimes.entry(1).or_default();
    runtime.receiver = Some(receiver);
    runtime.stage = Some(Stage::Provision);
    app.change_production_worker(1, Action::Rebuild, &ctx);
    let runtime = &app.cloud_prototype.production.runtimes[&1];
    assert!(runtime.rebuild.is_none());
    assert_eq!(runtime.stage, Some(Stage::Provision));
    assert!(runtime.error.is_none(), "settings are not read for a refused start");
}
