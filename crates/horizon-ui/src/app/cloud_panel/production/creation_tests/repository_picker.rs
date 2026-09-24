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

fn key_event(key: Key, pressed: bool, repeat: bool) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat,
        modifiers: Modifiers::NONE,
    }
}

fn hold(ctx: &egui::Context, app: &mut HorizonApp, held: Key) {
    frame(ctx, app, vec![key_event(held, true, false)], Modifiers::NONE);
    for _ in 0..6 {
        frame(ctx, app, vec![key_event(held, true, true)], Modifiers::NONE);
    }
}

fn release(ctx: &egui::Context, app: &mut HorizonApp, held: Key) {
    frame(ctx, app, vec![key_event(held, false, false)], Modifiers::NONE);
}

#[test]
fn holding_enter_on_the_repository_field_leaves_the_picker_open_and_empty() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    hold(&ctx, &mut app, Key::Enter);
    assert!(app.dir_picker.is_some(), "repeats must not confirm the picker");
    release(&ctx, &mut app, Key::Enter);
    assert!(app.dir_picker.is_some());
    assert!(app.cloud_prototype.production.repository.is_empty());
    assert!(app.cloud_creation_open());
}

#[test]
fn holding_escape_in_the_picker_keeps_the_dialog_open() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    app.cloud_prototype.production.title = "Held keys".into();
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert!(app.dir_picker.is_some());
    hold(&ctx, &mut app, Key::Escape);
    assert!(app.dir_picker.is_none(), "the first press cancels the picker");
    assert!(app.cloud_creation_open(), "repeats must not dismiss the dialog");
    release(&ctx, &mut app, Key::Escape);
    assert!(app.cloud_creation_open());
    assert_eq!(app.cloud_prototype.production.title, "Held keys");
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    assert!(!app.cloud_creation_open(), "a fresh Escape still dismisses the dialog");
}

#[test]
fn clicking_the_dialog_body_cancels_the_picker_and_keeps_the_dialog() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cancel = label_position(&output, "Cancel");
    click(&ctx, &mut app, label_position(&output, "Browse…"));
    assert!(app.dir_picker.is_some(), "the repository field opens the picker");
    click(&ctx, &mut app, cancel);
    assert!(
        app.dir_picker.is_none(),
        "a click on the disabled dialog cancels the picker"
    );
    assert!(app.cloud_creation_open(), "the disabled Cancel button does not act");
    assert!(app.cloud_prototype.production.repository.is_empty());
}

#[test]
fn choosing_another_repository_clears_its_stale_error() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let repository = temp.path().join("repository");
    std::fs::create_dir(&repository).unwrap();
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    app.cloud_prototype.error = Some("Cannot read .horizon/cloud.yml".into());
    app.set_cloud_repository(&repository);
    assert!(app.cloud_prototype.error.is_none());
    app.cloud_prototype.error = Some("unrelated".into());
    app.set_cloud_repository(&repository);
    assert_eq!(
        app.cloud_prototype.error.as_deref(),
        Some("unrelated"),
        "choosing the same repository keeps its error"
    );
}
