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
fn the_toolbar_has_one_button_for_a_cloud_and_one_for_an_empty_workspace() {
    let (_temp, ctx, mut app, workspaces) = ready_app();
    ctx.enable_accesskit();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cloud = label_position(&output, "+  New cloud");
    let workspace = label_position(&output, "+  Workspace");
    let labels = spoken(&output);
    for label in ["New cloud", "New workspace", "Menu"] {
        assert!(
            labels.iter().any(|(spoken, disabled)| spoken == label && !disabled),
            "{label}: {labels:?}"
        );
    }
    assert!(
        workspace.x > cloud.x && (workspace.y - cloud.y).abs() < 1.0,
        "+ Workspace follows + New cloud"
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

    // + New cloud makes a workspace and opens New cloud for it; Cancel takes it away.
    click(&ctx, &mut app, cloud);
    assert!(app.cloud_prototype.production.creating, "+ New cloud opens New cloud");
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(!app.cloud_creation_open());
    assert_eq!(app.board.workspaces.len(), workspaces, "the cancelled workspace goes");

    // + Workspace makes an empty workspace on this computer without a dialog or a menu.
    click(&ctx, &mut app, workspace);
    assert!(!app.cloud_prototype.production.creating, "+ Workspace opens no dialog");
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
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

    click(&ctx, &mut app, label_position(&output, "+  Workspace"));
    assert_eq!(app.board.workspaces.len(), workspaces + 1, "a This PC workspace");
}
