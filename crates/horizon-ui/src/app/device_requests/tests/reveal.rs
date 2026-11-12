use super::*;

#[test]
fn revealing_a_hidden_viewer_does_not_create_keyboard_focus() {
    let (_temp, ctx, mut app) = app();
    let create = request(
        &app,
        Operation::Create {
            frame_source: None,
            identity: None,
            endpoint: "127.0.0.1:5900".into(),
            ssh: None,
        },
    );
    let initial = one(app.apply_device_request(&create, &ctx));
    let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
    app.board.set_panel_visible(id, false);
    app.board.focused = None;
    let active = app.board.active_workspace;
    let reveal = request(
        &app,
        Operation::Reveal {
            panel_id: initial.panel_id,
        },
    );
    one(app.apply_device_request(&reveal, &ctx));
    assert!(app.board.panel(id).unwrap().visible);
    assert!(app.board.focused.is_none());
    assert_eq!(app.board.active_workspace, active);
}

#[test]
fn reveal_keeps_a_distant_viewer_on_screen_at_minimum_zoom() {
    let (_temp, ctx, mut app) = app();
    let create = request(
        &app,
        Operation::Create {
            frame_source: None,
            identity: None,
            endpoint: "127.0.0.1:5900".into(),
            ssh: None,
        },
    );
    let initial = one(app.apply_device_request(&create, &ctx));
    let id = app.board.panel_id_by_local_id(&initial.panel_id).unwrap();
    app.board.panel_mut(id).unwrap().layout.position = [100_000.0, 5000.0];
    let focused = app.board.focused;
    let reveal = request(
        &app,
        Operation::Reveal {
            panel_id: initial.panel_id,
        },
    );
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 900.0))),
            ..Default::default()
        },
        |ui| {
            let observed = one(app.apply_device_request(&reveal, ui.ctx()));
            app.apply_pending_root_device_reveal(ui.ctx());
            assert!(
                !observed.image.image_displayed,
                "acknowledgement is not proof of a painted image"
            );
            let canvas = app.canvas_rect(ui.ctx());
            let panel = app.board.panel(id).unwrap();
            let rect = egui::Rect::from_min_size(panel.layout.position.into(), panel.layout.size.into());
            let displayed = crate::app::view::canvas_scene_transform(canvas, app.canvas_view) * rect;
            assert!(
                canvas.contains_rect(displayed),
                "reveal left the viewer outside the canvas: {displayed:?} versus {canvas:?}"
            );
        },
    );
    output.textures_delta.clear();
    assert_eq!(app.board.focused, focused);
}
