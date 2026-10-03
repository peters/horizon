use super::*;
use crate::app::test_support::{editor_workspace_state, raw_input, test_app_with_startup};
use horizon_core::{RuntimeState, StartupDecision};

fn fixture() -> (tempfile::TempDir, Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState {
            workspaces: vec![editor_workspace_state("synthetic", [0.0, 0.0])],
            ..RuntimeState::default()
        }),
    });
    let panel = &app.board.panels[0];
    app.casting.picker = Some(Picker {
        anchor: panel.id,
        workspace: panel.workspace_id,
        source: CastSource::Panel {
            id: panel.local_id.clone(),
        },
        receiver: None,
        orientation: CastOrientation::Landscape,
        resolution: CastResolution::default(),
        pin: zeroize::Zeroizing::new(String::new()),
    });
    app.panel_screen_rects.insert(
        panel.id,
        Rect::from_min_size(Pos2::new(400.0, 140.0), Vec2::new(400.0, 300.0)),
    );
    (temp, ctx, app)
}

fn frame(ctx: &Context, app: &mut HorizonApp, events: Vec<egui::Event>) {
    let mut input = raw_input([1600.0, 1000.0], None);
    input.events = events;
    let _ = ctx
        .run_ui(input, |ui| app.render_cast_controls(ui.ctx()))
        .discard_textures();
}

#[test]
fn picker_tracks_its_icon_across_movement_zoom_and_reopening() {
    let (_temp, ctx, mut app) = fixture();
    let id = app.board.panels[0].id;
    for (position, zoom) in [(Pos2::new(400.0, 140.0), 1.0), (Pos2::new(700.0, 220.0), 1.6)] {
        app.canvas_view.zoom = zoom;
        let panel = Rect::from_min_size(position, Vec2::new(400.0, 300.0));
        app.panel_screen_rects.insert(id, panel);
        for _ in 0..4 {
            frame(&ctx, &mut app, Vec::new());
        }
        let popup = ctx.memory(|m| m.area_rect(Id::new("cast_picker"))).expect("picker");
        let icon = cast_icon_rect(panel, zoom);
        assert!((popup.right() - icon.right()).abs() < 1.0, "{popup:?}, {icon:?}");
        assert!((popup.top() - icon.bottom() - 8.0).abs() < 1.0, "{popup:?}, {icon:?}");
        assert!(ctx.memory(|m| m.area_rect(Id::new(("cast_icon", id.0)))).is_some());
        assert!(app.canvas_rect(&ctx).contains_rect(popup));
    }
}

#[test]
fn picker_is_nonmodal_but_excludes_its_own_controls() {
    let (_temp, ctx, mut app) = fixture();
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new());
    }
    assert!(!app.host_dialog_open());
    let popup = ctx.memory(|m| m.area_rect(Id::new("cast_picker"))).expect("picker");
    let exclusions = app.overlay_exclusion_zones(&ctx);
    assert!(exclusions.contains(popup.center()));
    assert!(!exclusions.contains(Pos2::new(1000.0, 700.0)));
    let menu = egui::LayerId::new(Order::Foreground, Id::new("synthetic-cast-menu"));
    app.casting.control_menus = Some([menu, menu]);
    for _ in 0..3 {
        let _ = ctx
            .run_ui(raw_input([1600.0, 1000.0], None), |ui| {
                app.render_cast_controls(ui.ctx());
                egui::Area::new(menu.id)
                    .order(menu.order)
                    .fixed_pos(Pos2::new(900.0, 500.0))
                    .show(ui.ctx(), |ui| {
                        ui.allocate_space(Vec2::new(100.0, 80.0));
                    });
                app.casting.control_menus = Some([menu, menu]);
            })
            .discard_textures();
    }
    assert!(app.overlay_exclusion_zones(&ctx).contains(Pos2::new(920.0, 520.0)));
    app.casting.picker = None;
    assert!(app.cast_control_screen_rects(&ctx).is_empty());
}

#[test]
fn clicking_the_visible_anchor_icon_closes_the_picker() {
    let (_temp, ctx, mut app) = fixture();
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new());
    }
    let id = app.board.panels[0].id;
    let pos = cast_icon_rect(app.panel_screen_rects[&id], app.canvas_view.zoom).center();
    for pressed in [true, false] {
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert!(app.casting.picker.is_none());
}

#[test]
fn losing_the_anchor_closes_the_picker() {
    let (_temp, ctx, mut app) = fixture();
    frame(&ctx, &mut app, Vec::new());
    app.panel_screen_rects.clear();
    frame(&ctx, &mut app, Vec::new());
    assert!(app.casting.picker.is_none());
}

#[test]
fn bottom_edge_keeps_the_picker_and_icon_visible() {
    let (_temp, ctx, mut app) = fixture();
    let id = app.board.panels[0].id;
    let panel = Rect::from_min_size(Pos2::new(400.0, 800.0), Vec2::new(400.0, 150.0));
    app.panel_screen_rects.insert(id, panel);
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new());
    }
    let popup = ctx.memory(|m| m.area_rect(Id::new("cast_picker"))).expect("picker");
    let icon = cast_icon_rect(panel, app.canvas_view.zoom);
    assert!(app.canvas_rect(&ctx).contains_rect(popup));
    assert!(popup.bottom() < icon.top(), "{popup:?}, {icon:?}");
}

#[test]
fn picker_wheels_do_not_pan_or_zoom_the_canvas_but_outside_wheels_do() {
    let (_temp, ctx, mut app) = fixture();
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new());
    }
    let popup = ctx.memory(|m| m.area_rect(Id::new("cast_picker"))).expect("picker");
    let pan = app.canvas_view.pan_offset;
    let zoom = app.canvas_view.zoom;
    for modifiers in [egui::Modifiers::NONE, egui::Modifiers::CTRL] {
        let mut input = raw_input([1600.0, 1000.0], None);
        input.events = vec![
            egui::Event::PointerMoved(popup.center()),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 50.0),
                phase: egui::TouchPhase::Start,
                modifiers,
            },
        ];
        let _ = ctx
            .run_ui(input, |ui| {
                app.handle_canvas_pan(ui.ctx());
                app.render_cast_controls(ui.ctx());
            })
            .discard_textures();
        assert_eq!(app.canvas_view.pan_offset.map(f32::to_bits), pan.map(f32::to_bits));
        assert_eq!(app.canvas_view.zoom.to_bits(), zoom.to_bits());
    }
    let mut input = raw_input([1600.0, 1000.0], None);
    input.events = vec![
        egui::Event::PointerMoved(Pos2::new(1100.0, 650.0)),
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -5.0),
            phase: egui::TouchPhase::Start,
            modifiers: egui::Modifiers::NONE,
        },
    ];
    let _ = ctx
        .run_ui(input, |ui| app.handle_canvas_pan(ui.ctx()))
        .discard_textures();
    assert_ne!(app.canvas_view.pan_offset.map(f32::to_bits), pan.map(f32::to_bits));
}
