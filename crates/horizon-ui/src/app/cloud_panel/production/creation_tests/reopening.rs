//! Reopening New cloud: its form is an ordinary surface on the workspace that holds the draft, so
//! the controls of the board around it stay usable and no click outside it takes it away.
use super::*;

#[test]
fn reopening_keeps_the_cloud_controls_outside_the_surface() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(temp.path().join("clouds"));
    app.cloud_prototype.ready = true;
    let workspace = app.board.create_workspace("existing workspace");
    for _ in 0..2 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let menu = label_position(&output, "Menu");
    let root = temp.path().join("clouds");
    app.cloud_prototype.root = Some(root.clone());
    let mut settings = horizon_core::cloud_runtime::setup::Draft::load(&root).unwrap();
    *settings.runpod_key = "synthetic-compute-key".into();
    settings.save().unwrap();
    app.cloud_prototype.groups.0.push(CloudGroup::new(
        1,
        "Existing cloud".into(),
        app.board.workspace(workspace).unwrap().local_id.clone(),
        temp.path().join("repository"),
        [4000.0, 4000.0],
    ));
    app.canvas_view.pan_offset[0] += 500.0;
    let original_view = app.canvas_view;
    app.cloud_overview(&ctx);
    assert_ne!(app.canvas_view, original_view, "Fit all must move this fixture");
    app.canvas_view = original_view;
    click(&ctx, &mut app, menu);
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let new_cloud = label_position(&output, "Add cloud here");
    click(&ctx, &mut app, new_cloud);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.cloud_prototype.production.creating {
        assert!(Instant::now() < deadline, "account check did not open creation");
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
        std::thread::yield_now();
    }
    frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    assert!(app.cloud_creation_open(), "toolbar click must open the form");
    assert_ne!(
        ctx.layer_id_at(menu).map(|layer| layer.id),
        Some(Id::unique("cloud-creation")),
        "the form is no modal: the toolbar keeps its own layer"
    );
    let view = app.canvas_view;
    click(&ctx, &mut app, menu);
    assert_eq!(app.canvas_view, view, "a click outside the form fits no canvas");
    assert!(
        app.cloud_creation_open(),
        "a click on a control outside the form leaves it"
    );
    assert!(app.cloud_prototype.production.draft.is_none(), "and parks nothing");
}
