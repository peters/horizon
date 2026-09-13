use super::*;
use crate::app::test_support::raw_input;
use crate::test_egui::DiscardTextures;

fn visible_text(shapes: &[egui::epaint::ClippedShape]) -> String {
    fn append(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(shape) => {
                text.push_str(shape.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    append(shape, text);
                }
            }
            _ => {}
        }
    }
    let mut text = String::new();
    for shape in shapes {
        append(&shape.shape, &mut text);
    }
    text
}

#[test]
fn both_provider_scopes_and_fresh_retry_consent_are_readable_without_dispatch() {
    for provider in [CloudProvider::Azure, CloudProvider::RunPod] {
        for size in [[1200.0, 900.0], [800.0, 600.0]] {
            let mut fixture = Fixture::new(provider);
            let ctx = Context::default();
            for request in [Action::Request, Action::Retry] {
                if matches!(request, Action::Retry) {
                    fixture.scope.expected.saved_phase = Some(requested());
                }
                let mut state = DeleteState::default();
                fixture.action(&mut state, request);
                let mut action = InventoryAction::None;
                let output = ctx.run_ui(raw_input(size, None), |ui| {
                    state.show(ui, &fixture.scope.expected, true, &mut action);
                });
                let text = visible_text(&output.shapes);
                let _ = output.discard_textures();
                for required in [
                    "synthetic-delete",
                    "Named provider profile",
                    "Unsaved memory",
                    "No checkpoint or backup",
                    "No SSH pin or private key",
                    "Cancel Delete",
                    "potential data loss",
                ] {
                    assert!(text.contains(required), "missing {required}: {text}");
                }
                if provider == CloudProvider::Azure {
                    for required in [
                        "entire owned Azure resource group",
                        "managed workspace data disk",
                        "/subscriptions/",
                    ] {
                        assert!(text.contains(required));
                    }
                    assert!(!text.contains("volume is not deleted"));
                } else {
                    for required in [
                        "only the exact RunPod Pod",
                        "remains separately billable",
                        "fresh Check/Retry cannot confirm bare absence",
                    ] {
                        assert!(text.contains(required));
                    }
                }
                if matches!(request, Action::Retry) {
                    assert!(text.contains("earlier Delete may still be in progress"));
                }
                assert!(matches!(action, InventoryAction::None));
                assert!(
                    !ctx.data(|data| data.get_temp::<(egui::Rect, bool)>(egui::Id::new("delete-confirm")))
                        .expect("confirm")
                        .1
                );
                assert!(state.pending.is_none());
            }
            assert!(!fixture.scope.home.root().exists());
        }
    }
}

#[test]
fn enter_and_space_cannot_activate_acknowledged_destructive_button() {
    let fixture = Fixture::new(CloudProvider::Azure);
    let ctx = Context::default();
    let mut state = DeleteState::default();
    fixture.action(&mut state, Action::Request);
    state.confirmation.as_mut().expect("consent").acknowledged = true;
    let _ = ctx
        .run_ui(raw_input([1200.0, 1100.0], None), |ui| {
            state.show(ui, &fixture.scope.expected, true, &mut InventoryAction::None);
        })
        .discard_textures();
    let button = ctx
        .data(|data| data.get_temp::<egui::Id>(egui::Id::new("delete-confirm-focus")))
        .expect("button ID");
    ctx.memory_mut(|memory| memory.request_focus(button));
    for key in [egui::Key::Enter, egui::Key::Space] {
        let mut input = raw_input([1200.0, 1100.0], None);
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        let mut action = InventoryAction::None;
        let _ = ctx
            .run_ui(input, |ui| state.show(ui, &fixture.scope.expected, true, &mut action))
            .discard_textures();
        assert!(matches!(action, InventoryAction::None));
        assert!(state.confirmation.is_some());
    }
}

#[test]
fn overview_keeps_all_management_buttons_disabled_during_delete_and_tombstones_static() {
    let fixture = Fixture::new(CloudProvider::RunPod);
    let ctx = Context::default();
    let mut view = fixture.view();
    let _tx = fixture.pending(&mut view.delete, Operation::Delete);
    let _ = ctx
        .run_ui(raw_input([1200.0, 1400.0], None), |ctx| {
            assert!(matches!(inventory_paint::show(ctx, &mut view), InventoryAction::None));
        })
        .discard_textures();
    for label in ["Delete environment…", "Check saved Delete", "Retry Delete…"] {
        assert!(
            !ctx.data(|data| data.get_temp::<(egui::Rect, bool)>(egui::Id::new(label)))
                .expect("button")
                .1
        );
    }
    assert_eq!(
        ctx.data(|data| data.get_temp::<bool>(egui::Id::new("stop-request-enabled-test"))),
        Some(false)
    );
    let mut tombstone = Fixture::new(CloudProvider::Azure);
    tombstone.scope.expected.saved_phase = Some(deleted());
    let mut state = DeleteState::default();
    let mut action = InventoryAction::None;
    let output = ctx.run_ui(raw_input([1000.0, 900.0], None), |ui| {
        state.show(ui, &tombstone.scope.expected, true, &mut action);
    });
    let text = visible_text(&output.shapes);
    let _ = output.discard_textures();
    assert!(text.contains("historical, not a new provider check"));
    assert!(matches!(action, InventoryAction::None));
}
