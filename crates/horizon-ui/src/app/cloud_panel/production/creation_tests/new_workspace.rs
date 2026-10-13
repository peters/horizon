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

#[test]
fn new_offers_the_cloud_first_above_the_sidebar_and_a_cancel_leaves_no_workspace() {
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
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "New"));
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let rects = |matches: &dyn Fn(&str) -> bool| {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) if matches(&text.galley.job.text) => {
                    Some(Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    // Under Cloud, the machine of a quick start and its price; the tests reach no provider.
    let machine = rects(&|text| text == super::super::new_workspace::machine::CHECKING);
    let [machine] = machine[..] else {
        panic!("one machine line under Cloud: {machine:?}");
    };
    // The toolbar and the sidebar show Cloud and This PC too: the menu's rows are the ones
    // aligned with the machine line, Cloud right above it and This PC below.
    let in_menu = |rect: &Rect| (rect.min.x - machine.min.x).abs() < 1.0;
    let cloud = rects(&|text| text == "Cloud")
        .into_iter()
        .filter(|rect| in_menu(rect) && rect.min.y < machine.min.y)
        .max_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .expect("a Cloud row above its machine line");
    let this_pc = rects(&|text| text == "This PC")
        .into_iter()
        .find(|rect| in_menu(rect) && rect.min.y > machine.min.y)
        .expect("a This PC row below Cloud");
    let rows = [cloud.center(), this_pc.center()];
    for row in rows {
        // Drawn above the sidebar, not under it.
        assert_eq!(ctx.layer_id_at(row).unwrap().order, egui::Order::Tooltip);
    }
    click(&ctx, &mut app, rows[0]);
    assert!(app.cloud_prototype.production.creating, "Cloud opens New cloud");
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(!app.cloud_creation_open());
    assert_eq!(app.board.workspaces.len(), workspaces, "the empty workspace goes");
}

#[test]
fn the_toolbar_split_button_makes_a_cloud_workspace_or_opens_the_place_menu() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    let _existing = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let workspaces = app.board.workspaces.len();
    ctx.enable_accesskit();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let create = label_position(&output, "+  New workspace");
    // The place part shows where the left part makes the workspace, also to a screen reader.
    let place = label_position(&output, "Cloud");
    let labels = spoken(&output);
    for label in ["New workspace on Cloud", "Choose where it runs, now Cloud", "Menu"] {
        assert!(labels.iter().any(|(spoken, _)| spoken == label), "{label}: {labels:?}");
    }
    assert!(
        place.x > create.x && (place.y - create.y).abs() < 1.0,
        "Cloud ▾ follows + New workspace"
    );

    // The left part makes a cloud workspace and opens New cloud for it.
    click(&ctx, &mut app, create);
    assert!(
        app.cloud_prototype.production.creating,
        "+ New workspace opens New cloud"
    );
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert_eq!(app.board.workspaces.len(), workspaces, "the cancelled workspace goes");

    // The place part opens the place menu; This PC makes a workspace without a dialog.
    click(&ctx, &mut app, place);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let this_pc = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "This PC" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        // The sidebar's This PC group is at the left; the menu opens under the toolbar's right.
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .filter(|position| position.x > place.x - 500.0)
        .expect("a This PC row in the place menu");
    click(&ctx, &mut app, this_pc);
    assert!(!app.cloud_prototype.production.creating, "This PC opens no dialog");
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
}

#[test]
fn without_cloud_workspaces_the_toolbar_makes_a_this_pc_workspace_and_offers_no_cloud() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let _existing = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    // Startup makes cloud workspaces ready; this computer has none.
    app.cloud_prototype.ready = false;
    let workspaces = app.board.workspaces.len();
    ctx.enable_accesskit();
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let create = label_position(&output, "+  New workspace");
    let place = label_position(&output, "PC");
    assert!(
        spoken(&output)
            .iter()
            .any(|(label, _)| label == "New workspace on This PC")
    );
    click(&ctx, &mut app, create);
    assert!(
        !app.cloud_prototype.production.creating,
        "no New cloud without cloud workspaces"
    );
    assert_eq!(app.board.workspaces.len(), workspaces + 1, "a This PC workspace");

    click(&ctx, &mut app, place);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    // The menu offers This PC and a dimmed Cloud row.
    let rows = spoken(&output);
    let row = |title: &str| {
        rows.iter()
            .find(|(label, _)| label.starts_with(&format!("{title}. ")))
            .unwrap_or_else(|| panic!("a {title} row: {rows:?}"))
            .1
    };
    assert!(row("Cloud"), "the Cloud row is disabled");
    assert!(!row("This PC"), "the This PC row can be chosen");
    // The Cloud row's title is aligned with the This PC title under it.
    let titles = |title: &str| {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.job.text == title => Some(text.pos),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let this_pc = titles("This PC")
        .into_iter()
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .expect("the This PC row");
    let cloud = titles("Cloud")
        .into_iter()
        .find(|position| (position.x - this_pc.x).abs() < 1.0 && position.y < this_pc.y)
        .expect("the Cloud row above This PC");
    let cloud = cloud + egui::vec2(10.0, 6.0);
    click(&ctx, &mut app, cloud);
    assert!(
        !app.cloud_prototype.production.creating,
        "the dimmed Cloud row does nothing"
    );
    assert_eq!(app.board.workspaces.len(), workspaces + 1);
}
