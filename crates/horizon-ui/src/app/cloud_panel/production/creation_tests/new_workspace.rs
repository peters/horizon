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
    let gpu = label_position(&output, "Cloud GPU");
    // The toolbar has a Cloud menu too: the menu's own row is the one right above Cloud GPU.
    let cloud = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.job.text == "Cloud" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .filter(|position| position.y < gpu.y && (position.x - gpu.x).abs() < 80.0)
        .max_by(|a, b| a.y.total_cmp(&b.y))
        .expect("a Cloud row above Cloud GPU");
    let rows = [cloud, gpu, label_position(&output, "This PC")];
    assert!(rows[0].y < rows[1].y && rows[1].y < rows[2].y, "the cloud comes first");
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
