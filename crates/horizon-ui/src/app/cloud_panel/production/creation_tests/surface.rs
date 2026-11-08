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
fn the_form_takes_the_window_and_stays_on_the_canvas() {
    let (_temp, ctx, mut app) = ready_app();
    let workspace = workspace_with_panel(&mut app, "sample project", [900.0, 700.0]);
    // The form starts on its workspace, so the workspace is in view first.
    app.focus_workspace_in_rect(workspace, app.canvas_rect(&ctx));
    app.open_workspace_cloud(&ctx, workspace);
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let surface = draft_workspace_frame(&app, &ctx);
    let canvas = app.canvas_rect(&ctx);
    let form = form_rect(&ctx).expect("the form is drawn");
    // A window with room beside a small workspace gives the form that room, so nothing scrolls
    // that the window could show. This window is narrower than the widest form, so the form
    // takes all of it.
    assert!(
        form.width() > surface.width(),
        "the form {form:?} is no wider than its workspace {surface:?}"
    );
    assert!(
        form.width() >= canvas.width() - 100.0,
        "the form {form:?} does not take the canvas {canvas:?}"
    );
    assert!(
        canvas.contains_rect(form),
        "the form stays on the canvas: {form:?} beside {canvas:?}"
    );
    // The body takes its height from the window, not from the workspace it is drawn on.
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let body = body_height(&output);
    let ceiling = super::super::creation::body_ceiling(ctx.content_rect());
    // The body follows the window, not the 700 px workspace it is drawn on.
    assert!(
        body > surface.height() - super::super::creation::CHROME_HEIGHT,
        "the body is {body} px tall in a workspace of {} px",
        surface.height()
    );
    assert!(
        body <= ceiling + 1.0,
        "the body is {body} px tall, above the ceiling {ceiling}"
    );
    // A pan moves the form while it can move, and the canvas keeps it inside.
    let pan = Vec2::new(120.0, 0.0);
    app.canvas_view.pan_offset[0] += pan.x;
    app.canvas_view.pan_offset[1] += pan.y;
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    let moved = form_rect(&ctx).expect("the form follows its workspace");
    assert!(
        moved.min.x >= form.min.x,
        "the form moved to {moved:?}, not from {:?}",
        form.min
    );
    assert!(
        app.canvas_rect(&ctx).contains_rect(moved),
        "the form stays on the canvas after a pan: {moved:?}"
    );
}

/// The body of the form, which the first field is clipped to.
fn body_height(output: &egui::FullOutput) -> f32 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "Cloud title" => Some(shape.clip_rect.height()),
            _ => None,
        })
        .expect("the body shows its first field")
}

#[test]
fn the_form_stops_growing_where_its_content_does() {
    use super::super::creation::{CHROME_HEIGHT, FORM_MAX_WIDTH, SETUP_MAX_WIDTH, body_ceiling, form_layout};
    let (_temp, ctx, mut app) = ready_app();
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    // A screen that is wider than the form can use.
    for _ in 0..6 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([5055.0, 1338.0], None));
    }
    let canvas = app.canvas_rect(&ctx);
    let form = form_rect(&ctx).expect("the form is drawn");
    assert!(
        form.width() <= crate::app::WS_CLOUD_FORM_SIZE[0],
        "the form {form:?} grew past the frame its workspace keeps for it"
    );
    assert!(
        form.width() < canvas.width() * 0.5,
        "the form {form:?} still follows the canvas {canvas:?} on a very large screen"
    );
    // The widths themselves, where no window hides which one the form took, and the height,
    // which still follows the window because a full form needs every row it can show.
    let room = Rect::from_min_size(Pos2::ZERO, Vec2::new(6000.0, 3000.0));
    for (widest, name) in [(FORM_MAX_WIDTH, "the form"), (SETUP_MAX_WIDTH, "a setup agent's form")] {
        let (width, body, _) = form_layout(room, room, body_ceiling(room), widest);
        assert!(
            (width - widest).abs() < 0.5,
            "{name} takes {width} px of room it cannot use, not {widest}"
        );
        assert!(
            (body - (room.height() - CHROME_HEIGHT)).abs() < 0.5,
            "{name} is {body} px tall, not the {} px the window leaves",
            room.height() - CHROME_HEIGHT
        );
    }
}

/// Draws `count` frames in a window of `size`.
fn frames_at(ctx: &egui::Context, app: &mut HorizonApp, size: [f32; 2], count: usize) {
    for _ in 0..count {
        run_app_frame_with_input(ctx, app, raw_input(size, None));
    }
}

#[test]
fn the_view_follows_the_window_so_the_form_stays_in_front() {
    let (_temp, ctx, mut app) = ready_app();
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    frames_at(&ctx, &mut app, [1400.0, 900.0], 6);
    let narrow = form_rect(&ctx).expect("the form is drawn");
    // The window becomes much wider than the form, which stops at its own width.
    frames_at(&ctx, &mut app, [2800.0, 1000.0], 6);
    let canvas = app.canvas_rect(&ctx);
    let wide = form_rect(&ctx).expect("the form is drawn in the wider window");
    assert!(
        wide.left() > narrow.left(),
        "the form stayed at {} while the canvas grew to {canvas:?}",
        narrow.left()
    );
    assert!(
        (wide.center().x - canvas.center().x).abs() < 120.0,
        "the form {wide:?} is not in the middle of the canvas {canvas:?}"
    );
    // A window that does not change leaves the view alone.
    frames_at(&ctx, &mut app, [2800.0, 1000.0], 3);
    let still = form_rect(&ctx).expect("the form stays drawn");
    assert!(
        (still.left() - wide.left()).abs() < 0.5,
        "the form moved to {still:?} without a resize, from {wide:?}"
    );
    // A pan of the person's own survives, because only the canvas moves the view.
    app.canvas_view.pan_offset[0] += 160.0;
    frames_at(&ctx, &mut app, [2800.0, 1000.0], 2);
    let panned = form_rect(&ctx).expect("the form follows its workspace");
    assert!(
        (panned.left() - still.left()).abs() > 1.0,
        "the pan was undone: the form is at {panned:?}, as before"
    );
}

#[test]
fn a_resize_in_another_workspace_keeps_the_view_and_the_keyboard() {
    let (_temp, ctx, mut app) = ready_app();
    let elsewhere = workspace_with_panel(&mut app, "sample project", [900.0, 700.0]);
    app.create_new_workspace(&ctx, NewWorkspace::Cloud, None);
    frames_at(&ctx, &mut app, [2800.0, 1000.0], 6);
    // The person leaves the draft and works in their own workspace, with the form still open.
    app.board.focus_workspace(elsewhere);
    frames_at(&ctx, &mut app, [2800.0, 1000.0], 2);
    let focused = app.board.focused;
    assert!(focused.is_some(), "the panel of that workspace has the keyboard");
    let before = form_rect(&ctx).expect("the form stays drawn");
    // Only the height changes, so the form keeps its width and any move is the view moving.
    frames_at(&ctx, &mut app, [2800.0, 760.0], 4);
    let after = form_rect(&ctx).expect("the form stays drawn");
    assert!(
        (after.left() - before.left()).abs() < 0.5,
        "the resize pulled the view back to the draft: {after:?}, from {before:?}"
    );
    assert_eq!(
        app.board.focused, focused,
        "the resize took the keyboard from the panel the person works in"
    );
    assert_eq!(
        app.board.active_workspace,
        Some(elsewhere),
        "the resize changed the workspace the person works in"
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

#[test]
fn a_detached_workspace_parks_the_draft_and_a_reopen_puts_the_form_on_the_canvas() {
    let (_temp, ctx, mut app) = ready_app();
    open_new_cloud(&ctx, &mut app);
    app.cloud_prototype.production.title = "Atlas cloud".into();
    let workspace = app
        .board
        .workspace_id_by_local_id(
            app.cloud_prototype
                .production
                .creation_workspace()
                .as_deref()
                .expect("the form belongs to a workspace"),
        )
        .expect("the workspace is on the board");
    // A workspace that moves to its own window cannot show the form, so the draft parks.
    app.detach_workspace(workspace);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(
        app.cloud_prototype.production.draft.is_some(),
        "the draft waits in its workspace"
    );
    assert!(!app.cloud_creation_open(), "no form is drawn while it waits");
    // The card kept the workspace, which shows it again when it rejoins the board.
    assert_eq!(
        app.cloud_prototype
            .production
            .draft
            .as_ref()
            .and_then(|draft| draft.workspace.clone()),
        app.board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone()),
        "the card keeps the workspace it belongs to"
    );
    // **+ New cloud** reopens the draft, and the form sits on the canvas rather than stay hidden.
    app.reopen_cloud_draft(&ctx);
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(app.cloud_creation_open(), "the reopened form is open");
    assert!(
        app.cloud_prototype.production.creation_workspace().is_none(),
        "the form forgets the workspace that cannot show it"
    );
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    assert!(has_label(&output, "Cloud title"), "the form draws on the canvas");
    assert!(has_label(&output, "Atlas cloud"), "with the choice it kept");
    // The workspace comes back to the board, which keeps the form where it is: the draft
    // forgot that workspace.
    app.reattach_workspace(&ctx, workspace);
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(
        app.cloud_prototype.production.creation_workspace().is_none(),
        "the form stays on the canvas"
    );
    // Cancel is reachable, so the draft cannot be stuck without a way out.
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cancel = label_position(&output, "Cancel");
    assert_eq!(
        order_at(&ctx, cancel),
        Some(egui::Order::Middle),
        "no overlay covers the footer of the form on the canvas"
    );
    click(&ctx, &mut app, cancel);
    assert!(
        !app.cloud_prototype.production.creating,
        "a click on the canvas form reaches Cancel"
    );
    assert!(app.cloud_prototype.production.draft.is_none(), "Cancel ends the draft");
}
