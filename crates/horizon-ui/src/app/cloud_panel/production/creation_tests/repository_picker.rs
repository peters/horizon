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

/// Types `text` into the focused source field.
fn type_text(ctx: &egui::Context, app: &mut HorizonApp, text: &str) {
    frame(ctx, app, vec![Event::Text(text.into())], Modifiers::NONE);
}

/// Draws frames until `text` shows, such as a finder row's title; folders are read on a
/// thread.
fn wait_for_text(ctx: &egui::Context, app: &mut HorizonApp, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
        let shown = output
            .shapes
            .iter()
            .any(|shape| matches!(&shape.shape, Shape::Text(shown) if shown.galley.job.text == text));
        if shown {
            return;
        }
        assert!(Instant::now() < deadline, "the finder never showed {text}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A link to a host that never resolves, so nothing reaches the network.
const LINK: &str = "https://git.invalid/demo-org/demo-atlas";

#[test]
fn keyboard_chooses_a_folder_result_and_keeps_focus_in_the_field() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let repository = temp.path().join("repository");
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    std::fs::create_dir(temp.path().join("plain-folder")).unwrap();
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    app.cloud_prototype.production.profiles = Some(
        CloudConfig::parse(
            "version: 1\ndefault: development\nprofiles:\n  development:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n",
        )
        .unwrap(),
    );
    type_text(&ctx, &mut app, &format!("{}/", temp.path().display()));
    wait_for_text(&ctx, &mut app, "repository");
    key(&ctx, &mut app, Key::Enter, Modifiers::NONE);
    assert!(app.dir_picker.is_none(), "no picker opens for a typed path");
    assert_eq!(app.cloud_prototype.production.repository, repository.to_string_lossy());
    assert!(
        app.cloud_prototype.production.profiles.is_none(),
        "a different repository needs its own profiles"
    );
    assert!(
        ctx.memory(|memory| memory.has_focus(Id::unique("cloud-source"))),
        "focus stays in the field after choosing"
    );
    app.cloud_prototype
        .production
        .source
        .edit_for_test(&format!("{}/plain", temp.path().display()));
    // Only the folder that matches is listed once this path's folders are read.
    wait_for_text(&ctx, &mut app, "1 FOLDER");
    key(&ctx, &mut app, Key::Tab, Modifiers::NONE);
    let opened = format!(
        "{}{}",
        temp.path().join("plain-folder").display(),
        std::path::MAIN_SEPARATOR
    );
    assert_eq!(
        app.cloud_prototype.production.source.input(),
        opened,
        "Tab opens the folder"
    );
    assert!(ctx.memory(|memory| memory.has_focus(Id::unique("cloud-source"))));
}

#[test]
fn enter_with_typed_text_in_one_frame_answers_the_text_not_the_rows_before_it() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    std::fs::create_dir_all(temp.path().join("repository/.git")).unwrap();
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    type_text(&ctx, &mut app, &format!("{}/", temp.path().display()));
    wait_for_text(&ctx, &mut app, "repository");
    // A paste and Enter in one frame: the highlighted row was made for the text before it.
    frame(
        &ctx,
        &mut app,
        vec![Event::Text("plain".into()), key_event(Key::Enter, true, false)],
        Modifiers::NONE,
    );
    assert!(
        app.cloud_prototype.production.repository.is_empty(),
        "the earlier row is not chosen"
    );
    assert_eq!(
        app.cloud_prototype.production.source.input(),
        format!("{}/plain", temp.path().display())
    );
}

/// Opens the dialog with a link in the field, so its clone folder can be chosen.
fn open_with_link(ctx: &egui::Context, app: &mut HorizonApp, root: std::path::PathBuf) {
    open_creation(ctx, app, root);
    type_text(ctx, app, LINK);
    frame(ctx, app, Vec::new(), Modifiers::NONE);
}

#[test]
fn enter_with_an_input_method_commit_in_one_frame_answers_the_text() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    std::fs::create_dir_all(temp.path().join("repository/.git")).unwrap();
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    type_text(&ctx, &mut app, &format!("{}/", temp.path().display()));
    wait_for_text(&ctx, &mut app, "repository");
    // An input method confirms its text and Enter in one frame, as CJK input methods do.
    frame(
        &ctx,
        &mut app,
        vec![
            Event::Ime(egui::ImeEvent::Commit("plain".into())),
            key_event(Key::Enter, true, false),
        ],
        Modifiers::NONE,
    );
    assert!(
        app.cloud_prototype.production.repository.is_empty(),
        "the earlier row is not chosen"
    );
    assert_eq!(
        app.cloud_prototype.production.source.input(),
        format!("{}/plain", temp.path().display())
    );
}

#[test]
fn clicking_outside_the_picker_cancels_it_and_keeps_the_dialog() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_with_link(&ctx, &mut app, temp.path().join("clouds"));
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "Choose folder…"));
    assert!(app.dir_picker.is_some(), "the clone folder opens the picker");
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
fn holding_enter_on_choose_folder_leaves_the_picker_open_and_empty() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_with_link(&ctx, &mut app, temp.path().join("clouds"));
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
    open_with_link(&ctx, &mut app, temp.path().join("clouds"));
    app.cloud_prototype.production.title = "Held keys".into();
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
    open_with_link(&ctx, &mut app, temp.path().join("clouds"));
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    let cancel = label_position(&output, "Cancel");
    click(&ctx, &mut app, label_position(&output, "Choose folder…"));
    assert!(app.dir_picker.is_some(), "the clone folder opens the picker");
    click(&ctx, &mut app, cancel);
    assert!(
        app.dir_picker.is_none(),
        "a click on the disabled dialog cancels the picker"
    );
    assert!(app.cloud_creation_open(), "the disabled Cancel button does not act");
    assert!(app.cloud_prototype.production.repository.is_empty());
    assert_eq!(app.cloud_prototype.production.source.input(), LINK, "the link stays");
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

#[test]
fn a_typed_folder_becomes_the_repository_and_names_the_cloud() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let repository = temp.path().join("demo-atlas");
    std::fs::create_dir_all(repository.join(".git")).unwrap();
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    let typed = std::time::Instant::now();
    frame(
        &ctx,
        &mut app,
        vec![Event::Text(repository.to_string_lossy().into())],
        Modifiers::NONE,
    );
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    // On a loaded computer the frames can outlast the pause that ends typing; only frames
    // drawn within it tell whether the path was taken too early.
    if typed.elapsed() < Duration::from_millis(400) {
        assert!(
            app.cloud_prototype.production.repository.is_empty(),
            "a path is not taken at the first repository it passes on the way to a longer one"
        );
    }
    std::thread::sleep(Duration::from_millis(600));
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert_eq!(
        std::path::Path::new(&app.cloud_prototype.production.repository),
        repository,
        "a folder that holds a repository is taken without a Continue once typing stops"
    );
    assert_eq!(app.cloud_prototype.production.title, "demo-atlas");
}

#[test]
fn a_pasted_link_waits_for_continue() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    frame(
        &ctx,
        &mut app,
        vec![Event::Text("github.com/demo-org/demo-atlas".into())],
        Modifiers::NONE,
    );
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(
        app.cloud_prototype.production.repository.is_empty(),
        "a link is cloned only when the person continues"
    );
    assert_eq!(
        app.cloud_prototype.production.source.input(),
        "github.com/demo-org/demo-atlas"
    );
}

#[test]
fn closing_the_dialog_forgets_what_was_typed_in_the_source_field() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    frame(
        &ctx,
        &mut app,
        vec![Event::Text("github.com/demo-org/demo-atlas".into())],
        Modifiers::NONE,
    );
    assert!(!app.cloud_prototype.production.source.input().is_empty());
    key(&ctx, &mut app, Key::Escape, Modifiers::NONE);
    assert!(!app.cloud_creation_open());
    assert!(app.cloud_prototype.production.source.input().is_empty());
}

#[test]
fn a_picker_closed_without_a_repository_reads_nothing() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    open_creation(&ctx, &mut app, temp.path().join("clouds"));
    app.cloud_prototype.production.picking_for_source = true;
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    assert!(
        app.cloud_prototype.error.is_none(),
        "no repository, so no error about reading one: {:?}",
        app.cloud_prototype.error
    );
}
