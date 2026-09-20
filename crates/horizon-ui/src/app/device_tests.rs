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

    let mut over_panel = raw_input([1400.0, 900.0], None);
    over_panel.events.push(egui::Event::PointerMoved(panel_rect.center()));
    over_panel.events.push(egui::Event::Zoom(1.25));
    run_app_frame_with_input(&ctx, &mut app, over_panel);
    assert_eq!(app.canvas_view, before, "the panel owns a pinch over its own frame");

    // The panel's own handler only covers its body, so the titlebar above it
    // stays with the canvas: ownership and handling share one rectangle.
    let titlebar = egui::pos2(panel_rect.center().x, panel_rect.top() + 4.0);
    let mut over_titlebar = raw_input([1400.0, 900.0], None);
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
    over_canvas.events.push(egui::Event::PointerMoved(empty));
    over_canvas.events.push(egui::Event::Zoom(1.25));
    run_app_frame_with_input(&ctx, &mut app, over_canvas);
    assert!(app.canvas_view.zoom > before.zoom, "empty canvas still zooms");
}
