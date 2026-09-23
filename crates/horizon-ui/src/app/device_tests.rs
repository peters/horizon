use egui::{Context, Shape};
use horizon_core::browser::manifest::device::{HostExclusion, HostPresentation, HostViewport};
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

fn host(app: &HorizonApp, panel: PanelId) -> HostPresentation {
    app.panel_render_caches.device_ui_state[&panel]
        .host
        .observation()
        .expect("host context")
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
    assert_eq!(host(&app, panel).exclusion, None);
    app.board.panel_mut(panel).expect("device").visible = false;
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    assert_eq!(host(&app, panel).exclusion, Some(HostExclusion::Hidden));
    app.board.panel_mut(panel).expect("device").visible = true;
    render(&ctx, &mut app);
    assert!(rendered(&app, panel));
    let position = app.board.panel(panel).expect("device").layout.position;
    app.board.panel_mut(panel).expect("device").layout.position = [100_000.0, 100_000.0];
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    assert_eq!(host(&app, panel).exclusion, Some(HostExclusion::OutsideCanvas));
    app.board.panel_mut(panel).expect("device").layout.position = position;
    let notes = app.board.panel_id_by_local_id("notes-panel").expect("notes");
    app.fullscreen_panel = Some(notes);
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    assert_eq!(host(&app, panel).exclusion, Some(HostExclusion::OtherPanelFullscreen));
    assert!(host(&app, panel).canvas.is_none());
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
    assert_eq!(host(&app, panel).exclusion, Some(HostExclusion::HostOverlay));
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
    assert_eq!(host(&app, panel).viewport, HostViewport::Detached);
    assert_eq!(host(&app, panel).exclusion, None);
    app.reattach_workspace(&ctx, workspace);
    app.fullscreen_panel = None;
    render(&ctx, &mut app);
    render(&ctx, &mut app);
    assert!(!app.workspace_is_detached(workspace));
    assert!(rendered(&app, panel), "reattached viewer did not resume");
}

#[test]
fn render_camera_survives_late_navigation_without_misattributing_collapse() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5901"));
    render(&ctx, &mut app);
    let workspace = app.board.panel(panel).unwrap().workspace_id;
    app.board.workspace_mut(workspace).unwrap().collapsed = true;
    render(&ctx, &mut app);
    assert!(
        rendered(&app, panel),
        "collapse metadata alone does not hide a visible panel"
    );
    assert_eq!(host(&app, panel).exclusion, None);
    let canvas = app.canvas_rect(&ctx);
    app.capture_root_device_presentation(canvas);
    let at_render = host(&app, panel).canvas;
    app.canvas_view.set_pan_offset([30_000.0, 20_000.0]);
    app.record_root_device_presentation(&ctx);
    let completed = host(&app, panel);
    assert_eq!(completed.canvas, at_render);
    assert_ne!(completed.canvas_after_pass, at_render);
    render(&ctx, &mut app);
    assert!(!rendered(&app, panel));
    assert_eq!(host(&app, panel).exclusion, Some(HostExclusion::OutsideCanvas));
}

#[test]
fn actual_discarded_pass_is_replaced_by_the_final_pass_context() {
    use crate::test_egui::DiscardTextures;
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5901"));
    render(&ctx, &mut app);
    let mut frame = eframe::Frame::_new_kittest();
    let mut passes = Vec::new();
    let _ = ctx
        .run_ui(raw_input([1400.0, 900.0], None), |ui| {
            if ui.ctx().current_pass_index() == 0 {
                ui.ctx().request_discard("synthetic viewer sizing pass");
            }
            eframe::App::ui(&mut app, ui, &mut frame);
            passes.push(host(&app, panel));
        })
        .discard_textures();
    assert!(passes.len() >= 2);
    assert!(passes[0].discarded);
    assert!(!passes.last().unwrap().discarded);
    assert!(passes.last().unwrap().ui_pass > passes[0].ui_pass);
    assert_eq!(host(&app, panel).ui_pass, passes.last().unwrap().ui_pass);
}
