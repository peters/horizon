use super::*;

fn open_creation(ctx: &egui::Context, app: &mut HorizonApp, root: std::path::PathBuf) {
    app.root_viewport_stabilizer = None;
    app.cloud_prototype.root = Some(root);
    app.cloud_prototype.ready = true;
    for _ in 0..10 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
    app.add_mock_cloud(ctx);
    for _ in 0..10 {
        frame(ctx, app, Vec::new(), Modifiers::NONE);
    }
}

#[test]
fn keyboard_chooses_a_typed_repository_and_returns_focus_to_the_field() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let repository = temp.path().join("repository");
    std::fs::create_dir(&repository).unwrap();
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    app.cloud_prototype.production.profiles = Some(
        CloudConfig::parse(
            "version: 1\ndefault: development\nprofiles:\n  development:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
        )
        .unwrap(),
    );
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert!(
        app.dir_picker.is_some(),
        "Enter opens the picker without also confirming it"
    );
    frame(
        &ctx,
        &mut app,
        vec![Event::Text(format!("{}/", repository.display()))],
        Modifiers::NONE,
    );
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert!(app.dir_picker.is_none());
    assert_eq!(app.cloud_prototype.production.repository, repository.to_string_lossy());
    assert!(
        app.cloud_prototype.production.profiles.is_none(),
        "a different repository needs its own profiles"
    );
    key(&ctx, &mut app, Key::Enter, Modifiers::COMMAND);
    assert!(
        app.dir_picker.is_some(),
        "focus returns to the field after choosing, and a modified Enter does not confirm"
    );
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    assert!(app.dir_picker.is_none());
    assert!(app.cloud_creation_open(), "Escape closes only the picker");
    assert_eq!(app.cloud_prototype.production.repository, repository.to_string_lossy());
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert!(app.dir_picker.is_some(), "focus also returns after cancelling");
}

#[test]
fn clicking_outside_the_picker_cancels_it_and_keeps_the_dialog() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "Browse…"));
    assert!(app.dir_picker.is_some(), "the repository field opens the picker");
    let outside = Pos2::new(8.0, 892.0);
    click(&ctx, &mut app, outside);
    assert!(app.dir_picker.is_none(), "an outside click cancels the picker");
    assert!(app.cloud_creation_open(), "the dialog stays open behind the picker");
    assert!(app.cloud_prototype.production.repository.is_empty());
    click(&ctx, &mut app, outside);
    assert!(
        !app.cloud_creation_open(),
        "the next outside click dismisses the dialog"
    );
}
