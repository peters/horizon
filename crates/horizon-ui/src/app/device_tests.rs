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
fn fixed_browser_pinches_zoom_the_canvas_while_wheels_remain_panel_owned() {
    for responsive in [false, true] {
        let (_temp, ctx, mut app, panel_id) = device_app(None);
        let browser = if responsive {
            horizon_core::browser::BrowserPanelState::inert()
        } else {
            horizon_core::browser::BrowserPanelState::inert_remote("target", "provider")
        };
        let panel = app.board.panel_mut(panel_id).expect("panel");
        panel.kind = PanelKind::Browser;
        panel.content = horizon_core::PanelContent::Browser(Box::new(browser));
        render(&ctx, &mut app);
        for (index, pinch) in [false, true, false].into_iter().enumerate() {
            let body = app
                .visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)
                .into_iter()
                .find(|(id, _)| *id == panel_id)
                .and_then(|(_, geometry)| geometry.zoom_body)
                .expect("browser body");
            let before = app.canvas_view.zoom;
            let mut input = raw_input([1400.0, 900.0], None);
            input.time = Some(1.0 + f64::from(u32::try_from(index).expect("small index")) * 0.02);
            input.events.push(egui::Event::PointerMoved(body.center()));
            if pinch {
                input.events.push(egui::Event::Zoom(1.25));
            } else {
                input
                    .events
                    .extend(
                        [egui::TouchPhase::Start, egui::TouchPhase::Move].map(|phase| egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: egui::vec2(0.0, 20.0),
                            phase,
                            modifiers: egui::Modifiers::CTRL,
                        }),
                    );
            }
            run_app_frame_with_input(&ctx, &mut app, input);
            if pinch && !responsive {
                assert!(app.canvas_view.zoom > before, "fixed browser swallowed native pinch");
            } else {
                assert!(
                    (app.canvas_view.zoom - before).abs() <= f32::EPSILON,
                    "panel-owned input leaked to canvas"
                );
            }
        }
    }
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
fn fullscreen_entry_discards_hidden_sidebar_gesture_hits() {
    for wheel in [false, true] {
        let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5903"));
        app.sidebar_visible = true;
        for _ in 0..3 {
            render(&ctx, &mut app);
        }
        let point = egui::pos2(20.0, 250.0);
        assert_eq!(
            ctx.layer_id_at(point),
            Some(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("sidebar")))
        );
        let before = app.canvas_view;
        app.fullscreen_panel = Some(panel);
        let mut input = raw_input([1400.0, 900.0], None);
        input.time = Some(1.0);
        input.events.push(egui::Event::PointerMoved(point));
        if wheel {
            input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 40.0),
                modifiers: egui::Modifiers::CTRL,
                phase: egui::TouchPhase::Move,
            });
        } else {
            input.events.push(egui::Event::Zoom(1.25));
        }
        run_app_frame_with_input(&ctx, &mut app, input);
        assert!(
            app.panel_render_caches.device_ui_state[&panel].zoom_factor() > 1.0,
            "wheel={wheel}"
        );
        assert_eq!(app.canvas_view, before);
    }
}

#[test]
fn fullscreen_entry_preserves_an_open_command_palette_gesture_blocker() {
    let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5903"));
    app.open_command_palette();
    for _ in 0..3 {
        render(&ctx, &mut app);
    }
    let point = egui::pos2(700.0, 250.0);
    assert_eq!(
        ctx.layer_id_at(point).map(|layer| layer.id),
        Some(egui::Id::new("palette_modal"))
    );
    let before = app.canvas_view;
    app.fullscreen_panel = Some(panel);
    let mut input = raw_input([1400.0, 900.0], None);
    input.time = Some(1.0);
    input.events = vec![egui::Event::PointerMoved(point), egui::Event::Zoom(1.25)];
    run_app_frame_with_input(&ctx, &mut app, input);
    assert!((app.panel_render_caches.device_ui_state[&panel].zoom_factor() - 1.0).abs() < f32::EPSILON);
    assert_eq!(app.canvas_view, before);
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
        assert_eq!(app.board.focused, None);
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
    for (top, expected) in [(notes, None), (panel, Some(panel))] {
        ctx.move_to_top(egui::LayerId::new(
            egui::Order::Middle,
            super::panels::panel_layer_salt(top),
        ));
        render(&ctx, &mut app);
        assert_eq!(app.board.focused, None);
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
            ctx.layer_id_at(point).map(|layer| layer.id),
            Some(super::panels::panel_layer_salt(top))
        );
        assert_eq!(app.zoom_gesture_panel(&ctx, None, &geometry, Some(point)), expected);
    }
}

#[test]
fn popup_gestures_do_not_zoom_the_covered_panel_or_canvas() {
    use crate::test_egui::DiscardTextures;
    for fullscreen in [false, true] {
        let (_temp, ctx, mut app, panel) = device_app(Some("127.0.0.1:5903"));
        render(&ctx, &mut app);
        render(&ctx, &mut app);
        let point = app
            .visible_panel_geometry_for_canvas_view(app.canvas_rect(&ctx), None)
            .iter()
            .find(|(id, _)| *id == panel)
            .expect("device")
            .1
            .zoom_body
            .expect("body")
            .center();
        app.fullscreen_panel = fullscreen.then_some(panel);
        let before = app.canvas_view;
        let popup = egui::Id::new("blocking-menu");
        for step in 0..4 {
            let mut input = raw_input([1400.0, 900.0], None);
            input.time = Some(f64::from(step) * 0.05);
            input.events.push(egui::Event::PointerMoved(point));
            if step >= 2 {
                input.events.push(egui::Event::Zoom(1.25));
            }
            let mut frame = eframe::Frame::_new_kittest();
            let _ = ctx
                .run_ui(input, |ui| {
                    eframe::App::ui(&mut app, ui, &mut frame);
                    egui::Popup::new(popup, ui.ctx().clone(), egui::Rect::NOTHING, ui.layer_id())
                        .at_position(point - egui::vec2(40.0, 40.0))
                        .kind(egui::PopupKind::Menu)
                        .open(true)
                        .show(|ui| {
                            ui.allocate_exact_size(egui::vec2(160.0, 160.0), egui::Sense::hover());
                        });
                })
                .discard_textures();
            if step >= 2 {
                assert_eq!(ctx.layer_id_at(point).map(|layer| layer.id), Some(popup));
                assert_eq!(app.canvas_view, before, "fullscreen={fullscreen}");
                assert!((app.panel_render_caches.device_ui_state[&panel].zoom_factor() - 1.0).abs() < f32::EPSILON);
            }
        }
    }
}
