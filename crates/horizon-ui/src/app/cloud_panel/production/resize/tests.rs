use super::*;
use crate::test_egui::DiscardTextures;

fn ready() -> Runtime {
    let mut state = super::super::rebuild::tests::deployment(
        Path::new("/synthetic"),
        false,
        super::super::rebuild::tests::Phase::None,
    );
    let mut worker = serde_json::to_value(state.worker.take().unwrap()).unwrap();
    worker["publicIp"] = serde_json::json!("192.0.2.1");
    worker["portMappings"] = serde_json::json!({"22":2200});
    state.worker = Some(serde_json::from_value(worker).unwrap());
    Runtime {
        state: Some(state),
        stage: Some(Stage::Ready),
        ..Runtime::default()
    }
}

fn frame(
    ctx: &egui::Context,
    runtime: &mut Runtime,
    events: Vec<egui::Event>,
) -> (Option<Action>, Vec<(String, egui::Pos2)>) {
    let mut action = None;
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 900.0))),
                events,
                ..Default::default()
            },
            |ui| {
                action = controls(ui, 1, runtime);
            },
        )
        .discard_textures();
    let labels = output
        .shapes
        .into_iter()
        .filter_map(|shape| match shape.shape {
            egui::epaint::Shape::Text(text) => {
                Some((text.galley.job.text.clone(), text.pos + text.galley.size() * 0.5))
            }
            _ => None,
        })
        .collect();
    (action, labels)
}

fn click(ctx: &egui::Context, runtime: &mut Runtime, label: &str) -> Option<Action> {
    let (_, labels) = frame(ctx, runtime, vec![]);
    let pos = labels
        .iter()
        .find(|(text, _)| text == label)
        .unwrap_or_else(|| panic!("Missing {label}: {labels:?}"))
        .1;
    frame(
        ctx,
        runtime,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame(
        ctx,
        runtime,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    )
    .0
}

#[test]
fn disk_growth_requires_review_and_confirmation_and_cancel_keeps_record() {
    let ctx = egui::Context::default();
    let mut runtime = ready();
    let original = runtime.state.as_ref().unwrap().profile.clone();
    assert!(click(&ctx, &mut runtime, "Grow workspace…").is_none());
    assert!(click(&ctx, &mut runtime, "Review resize…").is_none());
    let (_, labels) = frame(&ctx, &mut runtime, vec![]);
    assert!(labels.iter().any(|(text, _)| text.contains("cannot be undone")));
    assert!(click(&ctx, &mut runtime, "Cancel resize").is_none());
    assert!(runtime.resize.draft.is_none());
    assert_eq!(runtime.state.as_ref().unwrap().profile, original);
    click(&ctx, &mut runtime, "Grow workspace…");
    click(&ctx, &mut runtime, "Review resize…");
    assert_eq!(
        click(&ctx, &mut runtime, "Confirm resize"),
        Some(Action::Resize(ResizeTarget::Workspace {
            size_gb: original.storage.volume_gb + 10
        }))
    );
    assert_eq!(runtime.state.as_ref().unwrap().profile, original);
}

#[test]
fn compute_choices_are_reviewed_before_replacement() {
    let ctx = egui::Context::default();
    let mut runtime = ready();
    click(&ctx, &mut runtime, "Resize compute…");
    click(&ctx, &mut runtime, "4 vCPU");
    click(&ctx, &mut runtime, "8 vCPU");
    click(&ctx, &mut runtime, "Review resize…");
    let (_, labels) = frame(&ctx, &mut runtime, vec![]);
    assert!(
        labels
            .iter()
            .any(|(text, _)| text.contains("Container-disk files are lost"))
    );
    assert!(matches!(
        click(&ctx, &mut runtime, "Confirm resize"),
        Some(Action::Resize(ResizeTarget::Compute { cpu: 8, .. }))
    ));
}

#[test]
fn pending_recovery_offers_only_the_retained_target_even_without_a_snapshot() {
    let ctx = egui::Context::default();
    let target = ResizeTarget::Compute { cpu: 8, memory_gb: 16 };
    let mut runtime = Runtime {
        state_unavailable: true,
        ..Runtime::default()
    };
    runtime.resize.pending = Some(target);
    assert_eq!(click(&ctx, &mut runtime, "Retry resize"), Some(Action::Resize(target)));
    let (_, rx) = channel();
    runtime.resize.result = Some(Job {
        root: PathBuf::from("/synthetic"),
        receiver: rx,
    });
    assert!(runtime.busy());
    assert_eq!(click(&ctx, &mut runtime, "Retry resize"), None);
}

#[test]
fn stopped_gpu_and_non_ready_clouds_offer_no_resource_mutation() {
    let ctx = egui::Context::default();
    for kind in 0..3 {
        let mut runtime = ready();
        let state = runtime.state.as_mut().unwrap();
        match kind {
            0 => state.stop_requested = true,
            1 => state.profile.gpu = true,
            _ => state.stage = Stage::Provision,
        }
        assert!(frame(&ctx, &mut runtime, vec![]).1.is_empty());
    }
}

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn a_preflight_refusal_returns_the_healthy_cloud_to_its_watch_loop() {
    let root = tempfile::tempdir().unwrap();
    let state = ready().state.unwrap();
    Store::lock(root.path()).unwrap().save(&state).unwrap();
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file": root.path().join("missing-key"),
        "ssh_identity_file": root.path().join("missing-identity"),
        "docker_config": root.path().join("unused"), "cpu_flavors":["cpu3c"], "gpu_types":[]
    }))
    .unwrap();
    let cancel = cloud_runtime::Cancellation::default();
    let result = deployment::resize_compute(root.path(), &settings, 8, 16, &cancel, &|_| {});
    assert!(result.is_err());
    let events = std::cell::RefCell::new(Vec::new());
    let completion = complete(root.path(), result, &cancel, &|event| events.borrow_mut().push(event));
    let watched = completion.ready.expect("ready state restarts the watch loop");
    assert_eq!(watched.profile, state.profile);
    assert!(completion.outcome.notice.unwrap().starts_with("Resize refused:"));
    assert!(completion.outcome.pending.is_none());
    assert!(!completion.outcome.unavailable);
    assert!(events.borrow().iter().any(|event| matches!(event, Event::Ready(..))));
    assert!(!events.borrow().iter().any(|event| matches!(event, Event::Failed(..))));
}

#[test]
#[cfg_attr(windows, ignore = "Cloud records require Unix directory durability")]
fn disconnected_resize_worker_clears_busy_and_reloads_durable_state() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = ready();
    Store::lock(root.path())
        .unwrap()
        .save(runtime.state.as_ref().unwrap())
        .unwrap();
    for invalid_journal in [false, true] {
        if invalid_journal {
            std::fs::write(root.path().join("compute-resize.json"), "invalid").unwrap();
        }
        let (tx, rx) = channel();
        runtime.resize.result = Some(Job {
            root: root.path().to_owned(),
            receiver: rx,
        });
        runtime.stage = Some(Stage::Provision);
        let (events, receiver) = channel();
        runtime.sender = Some(events);
        runtime.receiver = Some(receiver);
        assert!(runtime.busy());
        runtime.poll_resize();
        assert!(runtime.busy(), "an empty connected channel is still running");
        drop(tx);
        runtime.poll_resize();
        assert!(!runtime.busy());
        assert!(runtime.receiver.is_none() && runtime.sender.is_none());
        assert_eq!(runtime.state_unavailable, invalid_journal);
        assert_eq!(runtime.state.is_some(), !invalid_journal);
        assert!(
            runtime
                .resize
                .notice
                .as_ref()
                .unwrap()
                .contains("ended without a result")
        );
        runtime.poll_resize();
        assert!(!runtime.busy());
    }
}
