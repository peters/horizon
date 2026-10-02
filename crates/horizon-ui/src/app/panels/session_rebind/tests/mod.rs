use super::*;
use crate::test_egui::DiscardTextures;
use egui::{Context, Event, FullOutput, Id, Key, Modifiers, PointerButton, Pos2, RawInput, Rect};
use horizon_core::PanelKind;

fn frame(ctx: &Context, events: Vec<Event>, launch: bool) -> FullOutput {
    let parent_id = Id::new("recovery_parent_menu");
    if launch {
        egui::Popup::open_id(ctx, parent_id);
    }
    ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 700.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            egui::Popup::new(
                parent_id,
                ctx.clone(),
                Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(10.0)),
                ui.layer_id(),
            )
            .open_memory(None)
            .show(|ui| {
                let response = ui.button("Resume a session…");
                if launch {
                    open_session_picker(
                        &response,
                        PanelId(1),
                        vec![AgentSessionBinding::new(
                            PanelKind::Codex,
                            "exact-session-id".into(),
                            None,
                            Some("Recovery test".into()),
                            None,
                        )],
                    );
                    ui.close();
                }
            });
            let binding = render_session_picker(
                ctx,
                PanelId(1),
                vec![AgentSessionBinding::new(
                    PanelKind::Codex,
                    "exact-session-id".into(),
                    None,
                    Some("Recovery test".into()),
                    None,
                )],
            );
            assert!(binding.is_none());
        },
    )
    .discard_textures()
}

fn text_center(output: &FullOutput, text: &str) -> Option<Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        egui::epaint::Shape::Text(shape) if shape.galley.job.text == text => {
            Some(shape.pos + shape.galley.size() / 2.0)
        }
        _ => None,
    })
}

mod input;
mod rendering;
