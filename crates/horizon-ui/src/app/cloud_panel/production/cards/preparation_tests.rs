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
