//! A new viewer panel must leave room for a 16:10 picture.

use super::*;

/// Draw a viewer panel of exactly `size` and report the body it left for the
/// picture. The panel's titlebar is not part of the widget's rect, so the test
/// subtracts it as the panel host does.
fn body_of_a_new_viewer(size: [f32; 2]) -> egui::Rect {
    let device = fixture_device();
    let mut state = ready_viewer();
    let ctx = egui::Context::default();
    let body_size = egui::vec2(size[0], size[1] - crate::app::PANEL_TITLEBAR_HEIGHT);
    let _ = ctx
        .run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, body_size)),
                ..Default::default()
            },
            |ui| state.show(ui, &device, true),
        )
        .discard_textures();
    state
        .image
        .body_rect
        .unwrap_or_else(|| panic!("a new viewer drew no body"))
}

#[test]
fn a_new_viewer_shows_a_sixteen_by_ten_desktop_at_its_default_size() {
    let default = horizon_core::DEFAULT_DEVICE_PANEL_SIZE;
    let body = body_of_a_new_viewer(default);
    assert!(
        body.width() >= 480.0,
        "a 16:10 desktop of 480 points wide needs 480 points of body, and the rows above the picture left {}: {:?}",
        body.width(),
        default
    );
    assert!(
        body.height() >= 300.0,
        "a 16:10 desktop of 480 points wide is 300 points high, and the rows above the picture left {}: {:?}",
        body.height(),
        default
    );
    assert!(
        (body.width() / body.height() - 1.6).abs() < 0.2 || body.height() >= 300.0,
        "the body holds a 16:10 picture: {body:?}"
    );
}
