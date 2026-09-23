use super::*;

#[test]
fn suppressed_or_fullscreen_frames_discard_pending_pan_and_zoom() {
    for fullscreen in [false, true] {
        let (_temp, ctx, mut app) = app_fixture();
        let pointer = Pos2::new(1350.0, 850.0);
        let mut input = scroll_frame(1.0, pointer, Vec2::ZERO, TouchPhase::Move);
        input
            .events
            .extend([Modifiers::NONE, Modifiers::CTRL].map(|modifiers| Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -1.0),
                phase: TouchPhase::Move,
                modifiers,
            }));
        let _ = run_app_frame_with_input(&ctx, &mut app, input);
        let pan = app.canvas_view.pan_offset;
        let zoom = app.canvas_view.zoom;
        if fullscreen {
            app.fullscreen_panel = Some(app.board.panels[0].id);
        }
        let _ = ctx
            .run_ui(scroll_frame(1.016, pointer, Vec2::ZERO, TouchPhase::End), |ui| {
                app.render_active_view(ui, !fullscreen);
            })
            .discard_textures();
        app.fullscreen_panel = None;
        let _ = ctx
            .run_ui(scroll_frame(1.032, pointer, Vec2::ZERO, TouchPhase::Move), |ui| {
                app.handle_canvas_pan(ui);
            })
            .discard_textures();
        assert_offset(app.canvas_view.pan_offset, pan);
        assert!((app.canvas_view.zoom - zoom).abs() < f32::EPSILON);
    }
}

#[test]
fn suppression_releases_a_phased_canvas_gesture_before_a_panel_move() {
    let (_temp, ctx, mut app) = app_fixture();
    let _ = run_app_frame_with_input(
        &ctx,
        &mut app,
        scroll_frame(1.0, Pos2::new(1350.0, 850.0), Vec2::new(0.0, -5.0), TouchPhase::Start),
    );
    let _ = ctx
        .run_ui(raw_input([1400.0, 900.0], None), |ui| {
            app.render_active_view(ui, true);
        })
        .discard_textures();
    let pointer = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)[0]
        .1
        .screen_rect
        .center();
    let pan = app.canvas_view.pan_offset;
    let _ = ctx
        .run_ui(
            scroll_frame(2.0, pointer, Vec2::new(0.0, -5.0), TouchPhase::Move),
            |ui| {
                app.handle_canvas_pan(ui);
            },
        )
        .discard_textures();
    assert_offset(app.canvas_view.pan_offset, pan);
    assert!(!app.canvas_pan_input_claimed);
}
