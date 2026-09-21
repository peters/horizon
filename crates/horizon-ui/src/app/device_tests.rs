use egui::{Context, Shape};
use horizon_core::{PanelId, PanelKind, RuntimeState, StartupChooser, StartupDecision, StartupPromptReason};

use super::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use super::{HorizonApp, StartupChooserState};

fn device_app(command: Option<&str>) -> (tempfile::TempDir, Context, HorizonApp, PanelId) {
    let mut workspace = editor_workspace_state("device", [0.0, 0.0]);
    workspace.panels[0].kind = PanelKind::Device;
    workspace.panels[0].command = command.map(str::to_owned);
    workspace.panels[0].size = Some([800.0, 450.0]);
    let runtime = RuntimeState {
        workspaces: vec![workspace, editor_workspace_state("notes", [900.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.root_viewport_stabilizer = None;
    let panel = app.board.panel_id_by_local_id("device-panel").expect("device panel");
    app.board.focused = Some(panel);
    (temp, ctx, app, panel)
}

fn render(ctx: &Context, app: &mut HorizonApp) -> egui::FullOutput {
    run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None))
}

fn rendered(app: &HorizonApp, panel: PanelId) -> bool {
    app.panel_render_caches
        .device_ui_state
        .get(&panel)
        .is_some_and(crate::device_widget::DeviceUiState::was_rendered)
}

fn append_text(shape: &Shape, text: &mut String) {
    match shape {
        Shape::Text(shape) => text.push_str(&shape.galley.job.text),
        Shape::Vec(shapes) => {
            for shape in shapes {
                append_text(shape, text);
            }
        }
        _ => {}
    }
}

#[test]
fn invalid_restored_device_displays_its_failure_transcript() {
    let (_temp, ctx, mut app, panel) = device_app(None);
    assert!(app.board.panel(panel).expect("placeholder").device().is_none());
    assert!(app.board.panel(panel).expect("placeholder").terminal().is_some());
    let mut text = String::new();
    for _ in 0..2 {
        for shape in render(&ctx, &mut app).shapes {
            append_text(&shape.shape, &mut text);
        }
    }
    assert!(
        text.contains("Device panel requires"),
        "restore failure was not painted: {text}"
    );
}

#[test]
fn viewer_visibility_tracks_hidden_off_canvas_fullscreen_and_startup_overlays() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5901"));
    render(&ctx, &mut app);
    assert!(rendered(&app, panel));
    app.board.panel_mut(panel).expect("device").visible = false;
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    app.board.panel_mut(panel).expect("device").visible = true;
    render(&ctx, &mut app);
    assert!(rendered(&app, panel));
    let position = app.board.panel(panel).expect("device").layout.position;
    app.board.panel_mut(panel).expect("device").layout.position = [100_000.0, 100_000.0];
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    app.board.panel_mut(panel).expect("device").layout.position = position;
    let notes = app.board.panel_id_by_local_id("notes-panel").expect("notes");
    app.fullscreen_panel = Some(notes);
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    app.fullscreen_panel = Some(panel);
    render(&ctx, &mut app);
    assert!(rendered(&app, panel));
    app.startup_chooser = Some(StartupChooserState {
        chooser: StartupChooser {
            reason: StartupPromptReason::MultipleRecoverable,
            config_path: "fixture.yaml".into(),
            sessions: Vec::new(),
        },
        selected_session_id: None,
        error: None,
    });
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    app.startup_chooser = None;
    render(&ctx, &mut app);
    assert!(rendered(&app, panel));
}

#[test]
fn detached_viewer_remains_active_during_root_fullscreen_and_reattachment() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5901"));
    render(&ctx, &mut app);
    let workspace = app.board.panel(panel).expect("device").workspace_id;
    app.detach_workspace(workspace);
    app.fullscreen_panel = app.board.panel_id_by_local_id("notes-panel");
    render(&ctx, &mut app);
    assert!(rendered(&app, panel), "root fullscreen suspended a detached viewer");
    app.reattach_workspace(&ctx, workspace);
    app.fullscreen_panel = None;
    render(&ctx, &mut app);
    render(&ctx, &mut app);
    assert!(!app.workspace_is_detached(workspace));
    assert!(rendered(&app, panel), "reattached viewer did not resume");
}

#[test]
fn a_pinch_over_a_device_panel_leaves_the_canvas_zoom_alone() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5902"));
    render(&ctx, &mut app);
    let canvas_rect = app.canvas_rect(&ctx);
    let geometry = app.visible_panel_geometry_for_canvas_view(canvas_rect, None);
    let panel_rect = geometry
        .iter()
        .find(|(id, _)| *id == panel)
        .expect("device panel geometry")
        .1
        .screen_rect;
    let before = app.canvas_view;

    // A new viewport has no previous paint order when its first input arrives.
    app.panel_screen_order.clear();
    let mut over_panel = raw_input([1400.0, 900.0], None);
    over_panel.events.push(egui::Event::PointerMoved(panel_rect.center()));
    over_panel.events.push(egui::Event::Zoom(1.25));
    run_app_frame_with_input(&ctx, &mut app, over_panel);
    assert_eq!(app.canvas_view, before, "the panel owns a pinch over its own frame");
    assert!((app.panel_render_caches.device_ui_state[&panel].zoom_factor() - 1.25).abs() < 0.001);

    // The panel's own handler only covers its body, so the titlebar above it
    // stays with the canvas: ownership and handling share one rectangle.
    let titlebar = egui::pos2(panel_rect.center().x, panel_rect.top() + 4.0);
    let mut over_titlebar = raw_input([1400.0, 900.0], None);
    // Past the gesture idle boundary, so this is a new gesture rather than a
    // continuation of the one the panel just claimed.
    over_titlebar.time = Some(10.0);
    over_titlebar.events.push(egui::Event::PointerMoved(titlebar));
    over_titlebar.events.push(egui::Event::Zoom(1.25));
    run_app_frame_with_input(&ctx, &mut app, over_titlebar);
    assert!(
        app.canvas_view.zoom > before.zoom,
        "the panel body does not cover its frame"
    );
    app.canvas_view = before;

    let empty = egui::pos2(canvas_rect.right() - 8.0, canvas_rect.bottom() - 8.0);
    assert!(
        geometry
            .iter()
            .all(|(_, geometry)| !geometry.screen_rect.contains(empty)),
        "the control point must be empty canvas"
    );
    let mut over_canvas = raw_input([1400.0, 900.0], None);
    over_canvas.time = Some(20.0);
    over_canvas.events.push(egui::Event::PointerMoved(empty));
    over_canvas.events.push(egui::Event::Zoom(1.25));
    run_app_frame_with_input(&ctx, &mut app, over_canvas);
    assert!(app.canvas_view.zoom > before.zoom, "empty canvas still zooms");
}

#[test]
fn a_zoom_gesture_continues_when_the_device_enters_fullscreen() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5902"));
    render(&ctx, &mut app);
    let body = app
        .visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)
        .into_iter()
        .find(|(id, _)| *id == panel)
        .and_then(|(_, geometry)| geometry.zoom_body)
        .expect("device body");
    let before = app.canvas_view;
    for (time, fullscreen, expected) in [(1.0, false, 1.25), (1.05, true, 1.5625), (1.1, false, 1.953_125)] {
        app.fullscreen_panel = fullscreen.then_some(panel);
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(time);
        input.events = vec![egui::Event::PointerMoved(body.center()), egui::Event::Zoom(1.25)];
        run_app_frame_with_input(&ctx, &mut app, input);
        let zoom = app.panel_render_caches.device_ui_state[&panel].zoom_factor();
        assert!(
            (zoom - expected).abs() < 0.001,
            "fullscreen={fullscreen}: {zoom} != {expected}"
        );
        assert_eq!(app.canvas_view, before);
    }
}

#[test]
fn paint_order_and_overlays_decide_which_panel_owns_a_zoom_gesture() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5903"));
    let notes = app.board.panel_id_by_local_id("notes-panel").expect("notes");
    let device_layout = app.board.panel(panel).expect("device").layout;
    if let Some(cover) = app.board.panel_mut(notes) {
        cover.layout.position = device_layout.position;
        cover.layout.size = device_layout.size;
    }
    render(&ctx, &mut app);
    let canvas_rect = app.canvas_rect(&ctx);
    let geometry = app.visible_panel_geometry_for_canvas_view(canvas_rect, None);
    let body = geometry
        .iter()
        .find(|(id, _)| *id == panel)
        .and_then(|(_, geometry)| geometry.zoom_body)
        .expect("device body");
    let point = body.center();

    // Both panels cover the point; the one painted last owns it, so a
    // terminal or editor covering a device panel keeps zoom on the canvas.
    assert!(
        geometry
            .iter()
            .filter(|(_, geometry)| geometry.screen_rect.contains(point))
            .count()
            >= 2,
        "the panels must overlap for this test to mean anything"
    );
    // Focus changes before routing, while the cached paint order still
    // describes the old frame. Both promotions must take effect immediately.
    app.panel_screen_order = vec![notes, panel];
    app.board.focused = Some(notes);
    assert_eq!(app.zoom_gesture_panel(&ctx, None, &geometry, Some(point)), None);
    app.panel_screen_order = vec![panel, notes];
    app.board.focused = Some(panel);
    assert_eq!(app.zoom_gesture_panel(&ctx, None, &geometry, Some(point)), Some(panel));

    // A detached window paints none of the root window's chrome, so a point
    // under the root sidebar is free for its panels.
    let sidebar = egui::pos2(canvas_rect.left() - 8.0, canvas_rect.center().y);
    let workspace = app.board.panel(panel).expect("device").workspace_id;
    assert!(app.overlay_exclusion_zones(&ctx).contains(sidebar));
    assert!(
        !app.overlay_exclusion_zones_for(&ctx, Some(workspace)).contains(sidebar),
        "a detached viewport must not inherit the root sidebar"
    );

    // A fixed overlay paints above every panel and keeps the gesture off it.
    let overlay = app
        .overlay_exclusion_zones(&ctx)
        .zones()
        .first()
        .copied()
        .expect("the fixture shows overlays");
    app.panel_screen_order = vec![panel];
    let covered = overlay.center();
    assert!(app.overlay_exclusion_zones(&ctx).contains(covered));
    assert_eq!(app.zoom_gesture_panel(&ctx, None, &geometry, Some(covered)), None);
}

#[test]
fn detached_zoom_routing_uses_current_workspace_order_without_a_paint_cache() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5903"));
    let notes = app.board.panel_id_by_local_id("notes-panel").expect("notes");
    let workspace = app.board.panel(panel).expect("device").workspace_id;
    app.board.assign_panel_to_workspace(notes, workspace);
    let device_layout = app.board.panel(panel).expect("device").layout;
    app.board.panel_mut(notes).expect("notes").layout = device_layout;
    render(&ctx, &mut app);
    let geometry = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), Some(workspace));
    let point = geometry
        .iter()
        .find(|(id, _)| *id == panel)
        .expect("device")
        .1
        .zoom_body
        .expect("body")
        .center();
    app.board.focused = None;
    app.panel_screen_order.clear();
    // The detached renderer uses workspace order, which can differ from the
    // board's root-window order after panel reassignment or rearrangement.
    let detached_ctx = Context::default();
    for (order, expected) in [(vec![panel, notes], None), (vec![notes, panel], Some(panel))] {
        app.board.workspace_mut(workspace).expect("workspace").panels = order;
        assert_eq!(
            app.zoom_gesture_panel(&detached_ctx, Some(workspace), &geometry, Some(point)),
            expected
        );
    }
}

#[test]
fn zoom_routing_follows_retained_layers_after_focus_leaves_overlapping_panels() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5903"));
    let notes = app.board.panel_id_by_local_id("notes-panel").expect("notes");
    let device_layout = app.board.panel(panel).expect("device").layout;
    app.board.panel_mut(notes).expect("notes").layout = device_layout;
    render(&ctx, &mut app);
    app.board.focused = None;
    for frame in 0..3 {
        let geometry = app.visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None);
        let point = geometry
            .iter()
            .find(|(id, _)| *id == panel)
            .expect("device")
            .1
            .zoom_body
            .expect("body")
            .center();
        assert_eq!(
            app.zoom_gesture_panel(&ctx, None, &geometry, Some(point)),
            Some(panel),
            "frame {frame}"
        );
        render(&ctx, &mut app);
        assert_eq!(
            ctx.layer_id_at(point).map(|layer| layer.id),
            Some(super::panels::panel_layer_salt(panel))
        );
    }
}
