use super::*;
use crate::app::cloud_panel::production::{Runtime, local_network::Sharing};
use horizon_core::cloud_runtime::deployment::ResizeTarget;

#[test]
fn pending_resize_keeps_the_paused_sharing_switch_in_connections() {
    let ctx = egui::Context::default();
    let mut runtime = Runtime {
        state_unavailable: true,
        sharing: Sharing::Paused { ready_again: false },
        ..Default::default()
    };
    runtime.resize.pending = Some(ResizeTarget::Compute { cpu: 8, memory_gb: 16 });
    let mut draw = |events| {
        let mut action = None;
        let output = ctx
            .run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    assert!(runtime_actions(ui, 1, &mut runtime).is_none());
                    action = super::super::drawer::access(ui, &mut runtime);
                },
            )
            .discard_textures();
        (action, output)
    };
    let (_, output) = draw(vec![]);
    let pos = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.job.text == "Share local network" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .expect("paused sharing stays visible during resize recovery");
    for pressed in [true, false] {
        let (action, _) = draw(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        if !pressed {
            assert_eq!(action, Some(Action::StopSharingLocalNetwork));
        }
    }
}
