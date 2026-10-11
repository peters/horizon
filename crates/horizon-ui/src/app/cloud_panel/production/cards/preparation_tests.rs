//! The card while a deployment prepares: the session and the record are saved off the
//! frame, and no action can start until the deployment does.
use super::*;
use crate::test_egui::DiscardTextures;
use horizon_core::cloud_runtime::state::Deployment;

#[test]
fn a_preparing_cloud_offers_no_action_until_its_deployment_starts() {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    let record = |stage: &str, operation: serde_json::Value| -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"prepare","repository":"/synthetic","revision":"a",
            "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":8,"memory_gb":32,"gpu":false},
            "stage":stage,"operation":operation,"spec":null,"worker":null,"sessions":[]
        }))
        .unwrap()
    };
    // A first deploy, a reconnect of a bound worker, and a resume of a stopped one.
    for (stage, state) in [
        (None, None),
        (
            Some(Stage::Provision),
            Some(record(
                "Provision",
                serde_json::json!({"state":"bound","worker_id":"worker1"}),
            )),
        ),
        (
            Some(Stage::Stopped),
            Some(record(
                "Stopped",
                serde_json::json!({"state":"bound","worker_id":"worker1"}),
            )),
        ),
    ] {
        let mut runtime = super::super::Runtime {
            stage,
            state,
            preparation: Some(super::super::preparation::Pending::running_for_test()),
            ..Default::default()
        };
        let header = status::of(&runtime, status::Occupancy::default(), std::time::SystemTime::now());
        assert_eq!(header.verb, "Preparing", "{stage:?}");
        assert!(header.primary.is_none(), "{stage:?}: the header offers no action");
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::Vec2::new(1200.0, 2000.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    assert!(runtime_actions(ui, 1, &mut runtime).is_none());
                },
            )
            .discard_textures();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text == "Saving the session and the deployment record…"),
            "{stage:?}: {texts:?}"
        );
        for label in [
            "Deploy cloud",
            "Reconnect cloud",
            "Resume worker",
            "Check provider",
            "Delete cloud resources…",
            "Remove cloud",
        ] {
            assert!(!texts.iter().any(|text| text == label), "{stage:?}: {label} is offered");
        }
    }
}

#[test]
fn a_retried_read_of_an_unreadable_record_says_it_is_preparing() {
    let runtime = super::super::Runtime {
        state_unavailable: true,
        preparation: Some(super::super::preparation::Pending::running_for_test()),
        ..Default::default()
    };
    let header = status::of(&runtime, status::Occupancy::default(), std::time::SystemTime::now());
    assert_eq!(header.verb, "Preparing");
    assert!(header.primary.is_none());
}

#[test]
#[cfg(unix)]
fn a_cloud_offers_no_lifecycle_action_while_a_session_save_runs() {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("saving");
    let record: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"saving","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":8,"memory_gb":32,"gpu":false},
        "stage":"Stopped","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let mut runtime = super::super::Runtime {
        stage: Some(Stage::Stopped),
        state: Some(record.clone()),
        ..Default::default()
    };
    let header = |runtime: &super::super::Runtime| {
        status::of(runtime, status::Occupancy::default(), std::time::SystemTime::now()).primary
    };
    assert!(header(&runtime).is_some(), "a stopped cloud offers Resume");
    runtime.sessions = super::super::session_record::Sessions::saving_for_test(&directory, record);
    assert!(
        header(&runtime).is_none(),
        "the header offers nothing while the save runs"
    );
    let output = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1200.0, 2000.0),
                )),
                ..Default::default()
            },
            |ui| {
                assert!(runtime_actions(ui, 1, &mut runtime).is_none());
            },
        )
        .discard_textures();
    super::super::session_record::Sessions::end_for_test(&directory);
    let texts: Vec<String> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text == "Saving the session and the deployment record…"),
        "{texts:?}"
    );
    for label in ["Resume worker", "Delete cloud resources…", "Check provider"] {
        assert!(
            !texts.iter().any(|text| text == label),
            "{label} is offered while the save runs"
        );
    }
}

#[test]
fn the_desktop_viewer_waits_for_a_preparation() {
    let mut record: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"desktop","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":8,"memory_gb":32,"gpu":false},
        "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    record.profile.capabilities.desktop = true;
    let runtime = super::super::Runtime {
        stage: Some(Stage::Ready),
        state: Some(record),
        preparation: Some(super::super::preparation::Pending::running_for_test()),
        ..Default::default()
    };
    assert_eq!(
        desktop_unavailable(&runtime),
        Some("Wait until the cloud has saved its session and record")
    );
}

#[test]
fn deletion_waits_for_the_operation_that_runs() {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, horizon_core::AppearanceTheme::Dark);
    let record: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"operating","repository":"/synthetic","revision":"a",
        "profile":{"provider":"runpod","image":"example.invalid/team/worker","cpu":8,"memory_gb":32,"gpu":false},
        "stage":"Provision","operation":{"state":"bound","worker_id":"worker1"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let shown = |runtime: &mut super::super::Runtime| -> Vec<String> {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::Vec2::new(1200.0, 2000.0),
                )),
                ..Default::default()
            },
            |ui| {
                let _ = runtime_actions(ui, 1, runtime);
            },
        )
        .discard_textures()
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_string()),
            _ => None,
        })
        .collect()
    };
    let (_sender, receiver) = std::sync::mpsc::channel();
    let mut runtime = super::super::Runtime {
        stage: Some(Stage::Provision),
        state: Some(record),
        receiver: Some(receiver),
        ..Default::default()
    };
    let texts = shown(&mut runtime);
    assert!(texts.iter().any(|text| text == "Cancel operation"), "{texts:?}");
    assert!(
        !texts.iter().any(|text| text == "Delete cloud resources…"),
        "a reconnect that runs offers no deletion: {texts:?}"
    );
    // A resize that reported Ready before its result arrived still holds the cloud.
    let (_sender, receiver) = std::sync::mpsc::channel();
    runtime.receiver = Some(receiver);
    runtime.stage = Some(Stage::Ready);
    runtime.resize = super::super::resize::State::running_for_test();
    let texts = shown(&mut runtime);
    assert!(
        !texts.iter().any(|text| text == "Delete cloud resources…"),
        "a resize that runs offers no deletion: {texts:?}"
    );
    runtime.resize = super::super::resize::State::default();
    runtime.receiver = None;
    runtime.stage = Some(Stage::Provision);
    let texts = shown(&mut runtime);
    assert!(texts.iter().any(|text| text == "Delete cloud resources…"), "{texts:?}");
}
