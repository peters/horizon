use super::*;

/// Texts drawn by one frame of the runtime actions.
fn action_texts(ctx: &egui::Context, runtime: &mut super::super::super::Runtime) -> Vec<String> {
    ctx.run_ui(egui::RawInput::default(), |ui| {
        runtime_actions(ui, 1, runtime);
    })
    .discard_textures()
    .shapes
    .iter()
    .filter_map(|shape| match &shape.shape {
        egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
        _ => None,
    })
    .collect()
}

/// A Ready cloud with a bound, running worker.
fn ready_bound_runtime() -> super::super::super::Runtime {
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"delete-fixture","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,
        "worker":{"id":"worker1","name":"delete-fixture","imageName":"registry.example/worker","desiredStatus":"RUNNING","costPerHr":0.74},
        "sessions":[]
    }))
    .unwrap();
    super::super::super::Runtime {
        stage: Some(Stage::Ready),
        state: Some(state),
        ..Default::default()
    }
}

fn has(texts: &[String], wanted: &str) -> bool {
    texts.iter().any(|text| text.starts_with(wanted))
}

#[test]
fn deletion_replaces_deployment_actions_with_its_steps_and_total_time() {
    use horizon_core::cloud_runtime::{Cancellation, progress::Progress};
    use std::time::{Duration, Instant};
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    let ready = action_texts(&ctx, &mut runtime);
    for shown in [
        "Delete cloud resources…",
        "Reconnect cloud",
        "Stop worker…",
        "Worker rate",
        "Provision worker",
    ] {
        assert!(has(&ready, shown), "{shown} is shown before deletion");
    }

    let (_sender, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    runtime.cancel = Some(Cancellation::default());
    runtime.stage = Some(Stage::ReleaseDevices);
    let pending = action_texts(&ctx, &mut runtime);
    assert!(pending.iter().any(|text| text == "Deleting cloud resources"));
    assert!(
        has(&pending, "Cancel operation"),
        "nothing irreversible was requested yet"
    );

    runtime.stage = Some(Stage::DeleteWorker);
    runtime.progress.stage(Stage::DeleteWorker, Instant::now());
    runtime
        .progress
        .update(Progress::activity("Deleting the worker and confirming its removal"));
    let deleting = action_texts(&ctx, &mut runtime);
    for shown in [
        "Deleting cloud resources · 0m",
        "Release hosted devices",
        "Delete worker · 0m",
        "Delete workspace storage",
        "Deleting the worker and confirming its removal",
    ] {
        assert!(has(&deleting, shown), "{shown} is shown while deleting");
    }
    for hidden in [
        "Cancel operation",
        "Delete cloud resources…",
        "Delete resources permanently",
        "Reconnect cloud",
        "Deploy cloud",
        "Stop worker…",
        "Worker rate",
        "Provision worker",
        "Check provider",
        "Redeploy cloud…",
    ] {
        assert!(!has(&deleting, hidden), "{hidden} is hidden while deleting");
    }

    // A failed deletion keeps its frozen steps beside the error and offers a retry.
    runtime.receiver = None;
    runtime.stage = Some(Stage::Ready);
    runtime.progress.finish(Instant::now());
    runtime.error = Some("Provider transport failed; reconcile before retrying".into());
    let failed = action_texts(&ctx, &mut runtime);
    for shown in [
        "Delete worker · 0m",
        "Deleting the worker and confirming its removal",
        "Provider transport failed",
        "Delete cloud resources…",
    ] {
        assert!(has(&failed, shown), "{shown} is shown after a failed deletion");
    }
    assert!(!has(&failed, "Provision worker"));
    assert!(!has(&failed, "Deleting cloud resources"));

    runtime.stage = Some(Stage::Deleted);
    runtime.progress.reset();
    let start = Instant::now().checked_sub(Duration::from_secs(30)).unwrap();
    runtime.progress.stage(Stage::DeleteWorker, start);
    runtime.progress.stage(Stage::Deleted, start + Duration::from_secs(12));
    let deleted = action_texts(&ctx, &mut runtime);
    for shown in [
        super::super::super::DELETED_RESOURCES_MESSAGE,
        "Deleted in 0m 12s",
        "Redeploy cloud…",
        "Remove cloud",
    ] {
        assert!(
            deleted.iter().any(|text| text == shown),
            "{shown} is shown after deletion"
        );
    }
}

#[test]
fn deletion_that_fails_before_its_first_step_keeps_its_steps() {
    // Without a requested worker, core fails before reporting a step and the saved
    // deployment stage comes back, but the card still presents a failed deletion.
    let ctx = egui::Context::default();
    let mut runtime = ready_bound_runtime();
    runtime.progress.begin_deletion();
    runtime.stage = Some(Stage::Provision);
    runtime.error = Some("No worker was requested".into());
    let early = action_texts(&ctx, &mut runtime);
    for shown in ["Release hosted devices", "Delete worker", "No worker was requested"] {
        assert!(has(&early, shown), "{shown} is shown after an early deletion failure");
    }
    assert!(!has(&early, "Provision worker"));
}
