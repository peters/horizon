//! The New cloud form is a surface on the workspace that holds its draft: where it sits, what a
//! click outside it and Escape leave, and how its draft waits when the workspace leaves the canvas.
use super::*;
use crate::app::sidebar::NewWorkspace;
use crate::app::view::canvas_scene_transform;
use egui::Vec2;

/// The form's own rect, while the layer that draws it showed the form in the last frame.
fn form_rect(ctx: &egui::Context) -> Option<Rect> {
    let id = Id::unique("cloud-creation");
    let layer = egui::LayerId::new(egui::Order::Middle, id);
    ctx.memory(|memory| {
        memory
            .areas()
            .visible_last_frame(&layer)
            .then(|| memory.area_rect(id))
            .flatten()
    })
}

/// Whether the frame drew `label`, for a form that must not show it.
fn has_label(output: &egui::FullOutput, label: &str) -> bool {
    output
        .shapes
        .iter()
        .any(|shape| matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text == label))
}

fn ready_app() -> (tempfile::TempDir, egui::Context, HorizonApp) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    (temp, ctx, app)
}

/// Opens New cloud through **+ New cloud**, which makes the workspace that holds the form.
fn open_new_cloud(ctx: &egui::Context, app: &mut HorizonApp) {
    app.create_new_workspace(ctx, NewWorkspace::Cloud, None);
    for _ in 0..4 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
    assert!(app.cloud_creation_open(), "the form is open on its workspace");
}

/// The on-screen frame of the workspace that holds the draft, which the form anchors on.
fn draft_workspace_frame(app: &HorizonApp, ctx: &egui::Context) -> Rect {
    let local = app
        .cloud_prototype
        .production
        .creation_workspace()
        .expect("the draft belongs to a workspace");
    let workspace = app
        .board
        .workspace_id_by_local_id(&local)
        .expect("the draft's workspace is on the board");
    let (position, size) = app.workspace_focus_frame(workspace).expect("the workspace frame");
    let canvas = app.canvas_rect(ctx);
    canvas_scene_transform(canvas, app.canvas_view) * Rect::from_min_size(position, size)
}

/// A workspace the person made, as big as `size` through the panel in it.
fn workspace_with_panel(app: &mut HorizonApp, name: &str, size: [f32; 2]) -> horizon_core::WorkspaceId {
    let workspace = app.board.create_workspace(name);
    let panel = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let layout = &mut app.board.panel_mut(panel).unwrap().layout;
    layout.position = [0.0, 0.0];
    layout.size = size;
    workspace
}

#[test]
fn the_surface_sits_on_the_draft_workspace_and_moves_with_a_pan() {
    let (_temp, ctx, mut app) = ready_app();
    let workspace = workspace_with_panel(&mut app, "sample project", [900.0, 700.0]);
    // The form sits on its workspace, so the workspace is in view first.
    app.focus_workspace_in_rect(workspace, app.canvas_rect(&ctx));
    app.open_workspace_cloud(&ctx, workspace);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let surface = draft_workspace_frame(&app, &ctx);
    let form = form_rect(&ctx).expect("the form is drawn on its workspace");
    // The form starts inside the workspace frame, below the name badge it must not cover.
    let inset = crate::app::WS_TITLE_HEIGHT + crate::app::WS_BG_PAD;
    assert!(
        (form.min.x - surface.min.x).abs() < 0.5 && (form.min.y - surface.min.y - inset).abs() < 0.5,
        "the form starts below the workspace's badge: {form:?} beside {surface:?}"
    );
    assert!(
        surface.contains_rect(form),
        "the form stays inside its workspace: {form:?} beside {surface:?}"
    );
    // The form fills the workspace that holds it, up to the margins its width rule keeps.
    assert!(
        form.width() <= surface.width() && form.width() >= surface.width() - 20.0,
        "the form {form:?} does not fill its workspace {surface:?}"
    );
    // The body takes its height from that workspace, below the workspace's name badge, not from
    // the window.
    let usable = surface.height() - crate::app::WS_TITLE_HEIGHT - crate::app::WS_BG_PAD;
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let body = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "Cloud title" => Some(shape.clip_rect.height()),
            _ => None,
        })
        .expect("the body shows its first field");
    assert!(
        (body - (usable - super::super::creation::CHROME_HEIGHT)).abs() < 0.5,
        "the body is {body} px tall in a workspace of {} px",
        surface.height()
    );
    // A pan moves the workspace on the screen, and the form with it.
    let pan = Vec2::new(120.0, 0.0);
    app.canvas_view.pan_offset[0] += pan.x;
    app.canvas_view.pan_offset[1] += pan.y;
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    let moved = form_rect(&ctx).expect("the form follows its workspace");
    assert!(
        (moved.min - (form.min + pan)).length() < 0.5,
        "the form moved to {moved:?}, not to {:?}",
        form.min + pan
    );
}

#[test]
fn a_workspace_the_person_made_keeps_its_size_and_the_form_keeps_its_floor() {
    let (_temp, ctx, mut app) = ready_app();
    let workspace = app.board.create_workspace("small workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.open_workspace_cloud(&ctx, workspace);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let surface = draft_workspace_frame(&app, &ctx);
    assert!(
        (surface.width() - crate::app::WS_EMPTY_SIZE[0]).abs() < 0.5,
        "a workspace the person made keeps its size: {surface:?}"
    );
    let form = form_rect(&ctx).expect("the form is drawn on its workspace");
    assert!(
        form.width() >= super::super::creation::FORM_MIN_WIDTH,
        "the form floors at a usable width: {form:?}"
    );
}

#[test]
fn the_form_never_leaves_the_canvas() {
    for size in [[1400.0, 900.0], [900.0, 700.0], [800.0, 600.0]] {
        let (_temp, ctx, mut app) = ready_app();
        app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
        for _ in 0..6 {
            let mut input = raw_input(size, None);
            input.events = Vec::new();
            run_app_frame_with_input(&ctx, &mut app, input);
        }
        let canvas = app.canvas_rect(&ctx);
        let form = form_rect(&ctx).unwrap_or_else(|| panic!("{size:?}: the form is drawn"));
        assert!(
            form.left() >= canvas.left() && form.top() >= canvas.top(),
            "{size:?}: the form {form:?} starts outside the canvas {canvas:?}"
        );
        assert!(
            form.bottom() <= canvas.bottom() && (form.right() <= canvas.right() || form.width() >= canvas.width()),
            "{size:?}: the form {form:?} leaves the canvas {canvas:?}"
        );
    }
}

#[test]
fn a_click_outside_the_surface_keeps_it_and_its_choices() {
    let (_temp, ctx, mut app) = ready_app();
    open_new_cloud(&ctx, &mut app);
    app.cloud_prototype.production.title = "Kept choice".into();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    // A control of the board, well outside the form.
    let outside = label_position(&output, "Menu");
    click(&ctx, &mut app, outside);
    assert!(app.cloud_creation_open(), "a click outside the form leaves it");
    assert!(app.cloud_prototype.production.draft.is_none(), "and parks nothing");
    assert_eq!(app.cloud_prototype.production.title, "Kept choice", "the choice stays");
    assert!(form_rect(&ctx).is_some(), "the form is still drawn");
}

#[test]
fn escape_neither_closes_the_surface_nor_drops_a_choice() {
    let (_temp, ctx, mut app) = ready_app();
    open_new_cloud(&ctx, &mut app);
    app.cloud_prototype.production.title = "Kept choice".into();
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    assert!(app.cloud_creation_open(), "Escape closes nothing");
    assert!(app.cloud_prototype.production.draft.is_none(), "Escape parks nothing");
    assert_eq!(app.cloud_prototype.production.title, "Kept choice", "the choice stays");
    assert!(form_rect(&ctx).is_some(), "the form is still drawn");
}

#[test]
fn a_new_cloud_workspace_that_lands_outside_the_view_draws_the_surface() {
    let (_temp, ctx, mut app) = ready_app();
    // A board wide enough that the workspace + New cloud makes lands outside the view.
    let _first = app.board.create_workspace("first workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let before = app.canvas_view;
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    for _ in 0..6 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(
        app.cloud_prototype.production.draft.is_none(),
        "a form that opens off the canvas parks nothing"
    );
    assert!(app.cloud_creation_open(), "the form is drawn");
    assert!(form_rect(&ctx).is_some(), "its layer holds the form");
    assert_ne!(
        app.canvas_view.pan_offset, before.pan_offset,
        "the view came to the workspace that holds the form"
    );
}

/// The order of the layer that owns a click at `point`.
fn order_at(ctx: &egui::Context, point: Pos2) -> Option<egui::Order> {
    ctx.layer_id_at(point).map(|layer| layer.order)
}

/// Whether the minimap overlay drew itself in the previous frame.
fn minimap_drawn(ctx: &egui::Context) -> bool {
    let layer = egui::LayerId::new(egui::Order::Foreground, Id::unique("minimap_overlay"));
    ctx.memory(|memory| memory.areas().visible_last_frame(&layer))
}

#[test]
fn the_form_owns_the_board_hud_while_it_is_drawn() {
    let (_temp, ctx, mut app) = ready_app();
    let _other = app.board.create_workspace("other workspace");
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(minimap_drawn(&ctx), "the minimap shows on a board");
    open_new_cloud(&ctx, &mut app);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cancel = label_position(&output, "Cancel");
    // The form's footer sits where the minimap would be, and the minimap is not drawn: the layer
    // that owns the footer corner is the form itself.
    assert!(!minimap_drawn(&ctx), "the minimap does not draw over the form");
    assert_eq!(
        order_at(&ctx, cancel),
        Some(egui::Order::Middle),
        "no HUD overlay is hosted above the form"
    );
    click(&ctx, &mut app, cancel);
    assert!(
        !app.cloud_prototype.production.creating,
        "a click at the footer reaches Cancel"
    );
    // The board's overlays come back over the next frames, once the click has settled.
    for _ in 0..6 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(minimap_drawn(&ctx), "the minimap comes back when the form is gone");
    assert_eq!(
        order_at(&ctx, cancel),
        Some(egui::Order::Foreground),
        "the minimap owns its corner again"
    );
}

#[test]
fn a_workspace_off_the_canvas_parks_the_draft_and_its_card_returns_the_form() {
    let (_temp, ctx, mut app) = ready_app();
    open_new_cloud(&ctx, &mut app);
    app.cloud_prototype.production.title = "Atlas cloud".into();
    // The workspace leaves the canvas, so the form cannot follow it.
    app.canvas_view.set_pan_offset([-100_000.0, -100_000.0]);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(
        app.cloud_prototype.production.draft.is_some(),
        "the draft waits in its workspace"
    );
    assert!(
        !app.cloud_creation_open(),
        "no surface is drawn while a draft is parked"
    );
    assert!(form_rect(&ctx).is_none(), "the layer draws no form");
    // Parking takes the view back to the workspace that holds the card.
    for _ in 0..20 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    assert!(!has_label(&output, "Cloud title"), "the parked draft draws no form");
    let continued = label_position(&output, "Continue");
    assert!(has_label(&output, "Atlas cloud"), "its card is the way back");
    click(&ctx, &mut app, continued);
    assert!(app.cloud_creation_open(), "Continue draws the form again");
    assert_eq!(
        app.cloud_prototype.production.title, "Atlas cloud",
        "with the choices it kept"
    );
    assert!(form_rect(&ctx).is_some(), "the form is drawn on its workspace");
}
