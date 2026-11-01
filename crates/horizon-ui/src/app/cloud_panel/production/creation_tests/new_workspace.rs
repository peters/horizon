use super::*;

/// The labels a screen reader gets in `output`, each with whether its control is disabled.
fn spoken(output: &egui::FullOutput) -> Vec<(String, bool)> {
    output
        .platform_output
        .accesskit_update
        .iter()
        .flat_map(|update| &update.nodes)
        .filter_map(|(_, node)| Some((node.label()?.to_owned(), node.is_disabled())))
        .collect()
}

/// The center of the text `label`, or nothing when no text says it.
fn text_center(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) if text.galley.job.text == label => {
            Some(Rect::from_min_size(text.pos, text.galley.size()).center())
        }
        _ => None,
    })
}

fn ready_app() -> (tempfile::TempDir, egui::Context, HorizonApp, usize) {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    // The board never removes its last workspace, so there is one already.
    let _existing = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let workspaces = app.board.workspaces.len();
    (temp, ctx, app, workspaces)
}

#[test]
fn new_in_the_sidebar_makes_an_empty_workspace_on_this_pc_without_a_menu() {
    let (_temp, ctx, mut app, workspaces) = ready_app();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "New"));
    assert!(!app.cloud_prototype.production.creating, "New opens no dialog");
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    assert!(
        text_center(&output, "This PC").is_some(),
        "the sidebar group of the new workspace"
    );
    assert!(text_center(&output, "Cloud").is_none(), "no menu offers a place");
}

#[test]
fn the_toolbar_has_one_button_for_a_cloud_and_the_menu() {
    let (_temp, ctx, mut app, workspaces) = ready_app();
    ctx.enable_accesskit();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cloud = label_position(&output, "+  New cloud");
    let labels = spoken(&output);
    for label in ["New cloud", "Menu"] {
        assert!(
            labels.iter().any(|(spoken, disabled)| spoken == label && !disabled),
            "{label}: {labels:?}"
        );
    }
    assert!(
        !labels.iter().any(|(spoken, _)| spoken == "New workspace"),
        "New workspace lives in the sidebar header only: {labels:?}"
    );
    let menu = label_position(&output, "Menu");
    assert!(
        menu.x > cloud.x && (menu.y - cloud.y).abs() < 1.0,
        "Menu follows + New cloud"
    );

    // The tooltip of + New cloud shows the machine and its price; the tests reach no
    // provider, so it says that it checks them.
    ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(cloud)], Modifiers::NONE);
    }
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = vec![Event::PointerMoved(cloud)];
    let output = run_app_frame_with_input(&ctx, &mut app, input);
    assert!(
        text_center(&output, super::super::new_workspace::machine::CHECKING).is_some(),
        "the machine line in the tooltip"
    );

    // + New cloud fills the empty workspace the person already has and opens New cloud on
    // it. The draft parks when that workspace leaves the canvas, and + New cloud brings it
    // back; the workspace the person made stays through Cancel.
    click(&ctx, &mut app, cloud);
    assert!(app.cloud_prototype.production.creating, "+ New cloud opens New cloud");
    assert_eq!(
        app.board.workspaces.len(),
        workspaces,
        "an empty workspace is filled instead of a second one being made"
    );
    // The form opens on its workspace, which the view has to reach first.
    let layer = egui::LayerId::new(egui::Order::Middle, Id::unique("cloud-creation"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ctx.memory(|memory| memory.areas().visible_last_frame(&layer)) {
        assert!(Instant::now() < deadline, "the form never drew on its workspace");
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    app.canvas_view.set_pan_offset([-100_000.0, -100_000.0]);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(!app.cloud_creation_open(), "a workspace off the canvas draws no form");
    assert!(
        app.cloud_prototype.production.draft.is_some(),
        "and parks the draft in its workspace"
    );
    assert_eq!(
        app.board.workspaces.len(),
        workspaces,
        "its workspace stays with the draft"
    );
    // Parking brings the view back to that workspace, where the card waits.
    for _ in 0..40 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cloud = label_position(&output, "+  New cloud");
    click(&ctx, &mut app, cloud);
    assert!(app.cloud_creation_open(), "+ New cloud brings the parked draft back");
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "Cancel"));
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(!app.cloud_prototype.production.creating, "Cancel ends the draft");
    assert_eq!(
        app.board.workspaces.len(),
        workspaces,
        "Cancel leaves the workspace the person already had"
    );
}

#[test]
fn without_cloud_workspaces_new_cloud_is_dimmed_and_does_nothing() {
    let (_temp, ctx, mut app, workspaces) = ready_app();
    // Startup makes cloud workspaces ready; this computer has none.
    app.cloud_prototype.ready = false;
    ctx.enable_accesskit();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cloud = label_position(&output, "+  New cloud");
    assert!(
        spoken(&output)
            .iter()
            .any(|(label, disabled)| label == "New cloud" && *disabled),
        "a disabled New cloud"
    );
    click(&ctx, &mut app, cloud);
    assert!(
        !app.cloud_prototype.production.creating,
        "no New cloud without cloud workspaces"
    );
    assert_eq!(app.board.workspaces.len(), workspaces);
}

#[test]
fn the_empty_canvas_offers_new_cloud_and_an_empty_workspace() {
    for (button, opens_new_cloud) in [("New Cloud", true), ("New Workspace", false)] {
        let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        });
        app.root_viewport_stabilizer = None;
        app.cloud_prototype.root = Some(temp.path().join("clouds"));
        app.cloud_prototype.ready = true;
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
        }
        assert!(app.board.workspaces.is_empty(), "the empty canvas needs an empty board");
        let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
        // All the card's buttons stay on one row.
        let rows = [
            "New Cloud",
            "New Workspace",
            "New Terminal",
            "Quick Nav",
            "Fit Workspace",
        ]
        .map(|label| label_position(&output, label).y);
        assert!(
            rows.iter().all(|y| (y - rows[0]).abs() < 1.0),
            "one row of buttons: {rows:?}"
        );
        click(&ctx, &mut app, label_position(&output, button));
        assert_eq!(app.board.workspaces.len(), 1, "{button} makes one workspace");
        assert_eq!(
            app.cloud_prototype.production.creating, opens_new_cloud,
            "{button} opens New cloud only for a cloud"
        );
    }
}

/// The id of the node that a screen reader reads as `label`, and the focused node.
fn node_and_focus(
    output: &egui::FullOutput,
    label: &str,
) -> (Option<egui::accesskit::NodeId>, egui::accesskit::NodeId) {
    let update = output
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("an AccessKit update");
    let node = update
        .nodes
        .iter()
        .find(|(_, node)| node.label() == Some(label))
        .map(|(id, _)| *id);
    (node, update.focus)
}

#[test]
fn a_dimmed_new_cloud_takes_no_focus_and_says_why_on_hover() {
    let tab = |ctx: &egui::Context, app: &mut HorizonApp| {
        let mut input = raw_input([1400.0, 900.0], None);
        input.events = vec![Event::Key {
            key: Key::Tab,
            physical_key: Some(Key::Tab),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }];
        run_app_frame_with_input(ctx, app, input)
    };
    // Whether Tab ever puts the focus on New cloud, in at most 40 steps.
    let focused_by_tab = |ready: bool| {
        let (_temp, ctx, mut app, _) = ready_app();
        app.cloud_prototype.ready = ready;
        ctx.enable_accesskit();
        (0..40).any(|_| {
            let output = tab(&ctx, &mut app);
            let (node, focus) = node_and_focus(&output, "New cloud");
            node == Some(focus)
        })
    };
    assert!(focused_by_tab(true), "Tab reaches a ready New cloud");
    assert!(!focused_by_tab(false), "Tab passes over a dimmed New cloud");

    let (_temp, ctx, mut app, _) = ready_app();
    app.cloud_prototype.ready = false;
    ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cloud = label_position(&output, "+  New cloud");
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(cloud)], Modifiers::NONE);
    }
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = vec![Event::PointerMoved(cloud)];
    let output = run_app_frame_with_input(&ctx, &mut app, input);
    assert!(
        text_center(&output, crate::app::sidebar::NEW_CLOUD_NOT_READY).is_some(),
        "the hover explains the dimmed button"
    );
}

#[test]
fn new_cloud_fills_an_empty_workspace_instead_of_leaving_another_behind() {
    let (_temp, ctx, mut app, workspaces) = ready_app();
    assert_eq!(workspaces, 1, "the board holds one empty workspace");
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "+  New cloud"));
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(app.cloud_prototype.production.creating, "New cloud opens the dialog");
    assert_eq!(
        app.board.workspaces.len(),
        1,
        "the cloud takes the empty workspace instead of making a second one"
    );
    assert_eq!(
        app.cloud_prototype.production.creation_workspace().as_deref(),
        Some(app.board.workspaces[0].local_id.as_str()),
        "the dialog points at the workspace the person already has"
    );
}

#[test]
fn new_cloud_still_makes_a_workspace_when_the_board_holds_panels_only() {
    let (_temp, ctx, mut app, workspaces) = ready_app();
    let workspace = app.board.workspaces[0].id;
    app.board
        .create_panel(
            horizon_core::PanelOptions {
                name: Some("a panel".to_owned()),
                kind: horizon_core::PanelKind::Editor,
                command: Some("seed".to_owned()),
                ..horizon_core::PanelOptions::default()
            },
            workspace,
        )
        .unwrap();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "+  New cloud"));
    assert!(app.cloud_prototype.production.creating);
    assert_eq!(
        app.board.workspaces.len(),
        workspaces + 1,
        "a workspace that holds a panel is not taken for a new cloud"
    );
}
