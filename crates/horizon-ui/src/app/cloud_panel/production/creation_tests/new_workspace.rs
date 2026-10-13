use super::*;

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
