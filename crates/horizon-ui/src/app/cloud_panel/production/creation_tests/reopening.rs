use super::*;

#[test]
fn reopening_keeps_cloud_controls_below_the_modal() {
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
    for _ in 0..2 {
        click(&ctx, &mut app, menu);
        let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
        click(&ctx, &mut app, label_position(&output, "Cloud"));
        let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
        let new_cloud = label_position(&output, "New cloud…");
        click(&ctx, &mut app, new_cloud);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.cloud_prototype.production.creating {
            assert!(Instant::now() < deadline, "account check did not open creation");
            frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
            std::thread::yield_now();
        }
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
        assert!(app.cloud_creation_open(), "toolbar click must open the dialog");
        assert_eq!(ctx.layer_id_at(menu).unwrap().id, Id::new("cloud-creation"));
        for _ in 0..16 {
            key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
            if let Some(response) = ctx.memory(egui::Memory::focused).and_then(|id| ctx.read_response(id)) {
                assert_eq!(response.layer_id.id, Id::new("cloud-creation"));
            }
        }
        let view = app.canvas_view;
        click(&ctx, &mut app, menu);
        assert_eq!(app.canvas_view, view, "backdrop click must not fit the canvas");
        assert!(!app.cloud_creation_open(), "backdrop click dismisses the dialog");
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
}

#[test]
fn creation_dialog_and_actions_fit_after_shrinking_with_scrolling_content() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    for _ in 0..3 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([3840.0, 2140.0], None));
    }
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.error = Some("Configuration needs correction before deployment.\n".repeat(30));
    // Before a repository is chosen the dialog offers Continue; after, Start cloud.
    for (repository, action) in [("", "Continue"), ("/work/atlas", "Start cloud")] {
        app.cloud_prototype.production.repository = repository.into();
        for size in [[3840.0, 2140.0], [900.0, 700.0], [800.0, 600.0]] {
            for _ in 0..8 {
                run_app_frame_with_input(&ctx, &mut app, raw_input(size, None));
            }
            let output = run_app_frame_with_input(&ctx, &mut app, raw_input(size, None));
            let dialog = ctx
                .memory(|memory| memory.area_rect(Id::new("cloud-creation")))
                .unwrap();
            let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0], size[1]));
            assert!(
                viewport.contains_rect(dialog),
                "{size:?}: dialog {dialog:?} exceeds {viewport:?}"
            );
            for label in [action, "Cancel"] {
                let shape = output
                    .shapes
                    .iter()
                    .find(|shape| matches!(&shape.shape, Shape::Text(text) if text.galley.job.text == label))
                    .unwrap();
                let Shape::Text(text) = &shape.shape else {
                    unreachable!()
                };
                let bounds = Rect::from_min_size(text.pos, text.galley.size());
                assert!(viewport.contains_rect(bounds));
                assert!(shape.clip_rect.contains_rect(bounds), "{label} is clipped at {size:?}");
            }
        }
    }
}

/// Where `label` was drawn and the clip it was drawn in, when the frame drew it.
fn drawn(output: &egui::FullOutput, label: &str) -> Option<(Pos2, Rect)> {
    output.shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) if text.galley.job.text == label => Some((text.pos, shape.clip_rect)),
        _ => None,
    })
}

#[test]
fn creation_body_fills_the_body_height_in_one_and_two_columns() {
    let (_temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.root_viewport_stabilizer = None;
    let mut time = 0.0;
    let mut heights = Vec::new();
    // Two columns, then one, then two again: each opening is measured in its own window, and
    // only there. Opening again in the same window draws the dialog on its first frame.
    let sizes = [[1600.0, 1000.0], [800.0, 900.0], [1600.0, 900.0], [1600.0, 900.0]];
    for (index, size) in sizes.into_iter().enumerate() {
        let mut frame = |app: &mut HorizonApp| {
            time += 1.0 / 60.0;
            let mut input = raw_input(size, None);
            input.time = Some(time);
            let output = run_app_frame_with_input(&ctx, app, input);
            let dialog = ctx.memory(|memory| memory.area_rect(Id::new("cloud-creation")));
            (drawn(&output, "Cloud title"), drawn(&output, "Cancel"), dialog)
        };
        for _ in 0..2 {
            frame(&mut app);
        }
        app.cloud_prototype.production.creating = true;
        app.set_cloud_repository(std::path::Path::new("/work/atlas"));
        // Only the opening frame may measure the dialog without drawing it.
        let mut first = frame(&mut app);
        if first.0.is_none() {
            assert!(
                !sizes[..index].contains(&size),
                "{size:?}: the dialog was measured again in the same window"
            );
            first = frame(&mut app);
        }
        let (Some((_, body)), Some(_), Some(dialog)) = first else {
            panic!("{size:?}: the first visible frame drew {first:?}");
        };
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(size[0], size[1]));
        // The text is clipped to the scroll area it scrolls in, whichever column that is.
        let expected = super::super::creation::body_height(viewport);
        assert!(
            (body.height() - expected).abs() < 0.5,
            "{size:?}: the body scroll area is {body:?}, not {expected} px tall"
        );
        assert!(
            viewport.contains_rect(dialog),
            "{size:?}: dialog {dialog:?} exceeds {viewport:?}"
        );
        for _ in 0..30 {
            assert_eq!(frame(&mut app), first, "{size:?}: the dialog changed after it opened");
        }
        heights.push(dialog.height());
        app.close_cloud_creation();
    }
    // At one window height (900 px), one column makes a dialog as tall as two columns do.
    let [two, one, two_again, _] = heights[..] else {
        unreachable!()
    };
    assert!(two > two_again, "a shorter window must make a shorter dialog");
    assert!(
        (one - two_again).abs() < 0.5,
        "one column is {one} px tall, two columns {two_again} px"
    );
}
