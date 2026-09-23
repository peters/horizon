use super::*;

#[test]
fn empty_or_edge_touching_image_never_counts_as_displayed() {
    let ctx = egui::Context::default();
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            let texture = ui
                .ctx()
                .load_texture("fixture", patterned_desktop(), TextureOptions::LINEAR);
            assert!(!visible_image(ui, &texture, egui::Vec2::ZERO, false).0);
            let top = ui.next_widget_position();
            ui.set_clip_rect(egui::Rect::from_min_max(top - egui::vec2(50.0, 50.0), top));
            assert!(!visible_image(ui, &texture, egui::vec2(100.0, 100.0), false).0);
        })
        .discard_textures();
}

#[test]
fn invisible_sizing_ui_never_counts_as_displayed_image() {
    let ctx = egui::Context::default();
    let device = fixture_device();
    let mut state = DeviceUiState {
        initialized: true,
        status: Status::Connected,
        ..Default::default()
    };
    let _ = ctx
        .run_ui(egui::RawInput::default(), |ui| {
            state.update_texture(ui, patterned_desktop());
            ui.set_invisible();
            state.show(ui, &device, true);
            assert!(state.image.received);
            assert!(!state.image.displayed);
            assert!(state.image.last_displayed.is_none());
        })
        .discard_textures();
}
