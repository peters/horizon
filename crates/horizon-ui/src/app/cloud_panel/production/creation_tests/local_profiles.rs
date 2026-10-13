use super::*;
use horizon_core::cloud_runtime::repository::launch::{Configuration, quick_start};

const CONFIG: &str = "version: 1\ndefault: test-image\nprofiles:\n  test-image:\n    provider: runpod\n    image: example.invalid/worker:test\n    min_cpu: 4\n    min_memory_gb: 8\n    capabilities:\n      agents: []\n      browsers: []\n      desktop: true\n";

fn fixture(path: &std::path::Path) {
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Committed application",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    std::fs::create_dir(path.join(".horizon")).unwrap();
    std::fs::write(path.join(".horizon/cloud.yml"), CONFIG).unwrap();
}

fn finish_read(ctx: &egui::Context, app: &mut HorizonApp) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.cloud_prototype.production.launch.loading() {
        assert!(Instant::now() < deadline, "Configuration read did not finish");
        app.poll_cloud_launch(ctx);
        std::thread::yield_now();
    }
}

/// Clicks `label` in a window tall enough to show the whole dialog body, once a section
/// opened by an earlier click has finished its animation.
fn tall_click(ctx: &egui::Context, app: &mut HorizonApp, label: &str) {
    let tall = || raw_input([1400.0, 2400.0], None);
    for _ in 0..12 {
        run_app_frame_with_input(ctx, app, tall());
    }
    let at = label_position(&run_app_frame_with_input(ctx, app, tall()), label);
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
        vec![egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    ] {
        let mut input = tall();
        input.events = events;
        run_app_frame_with_input(ctx, app, input);
    }
}

fn wait_until(ctx: &egui::Context, app: &mut HorizonApp, done: impl Fn(&HorizonApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(Instant::now() < deadline, "the dialog did not read the change");
        app.poll_cloud_launch(ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn painted(output: &egui::FullOutput) -> String {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) => Some(text.galley.job.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn untracked_settings_load_without_a_button_and_the_profile_is_captured_for_creation() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.repository = temp.path().to_string_lossy().into();
    app.cloud_prototype.production.title = "Test image".into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    let form = &mut app.cloud_prototype.production;
    assert_eq!(form.launch.configuration, Configuration::WorkingCopy);
    assert!(!form.launch.unconfigured);
    assert!(form.launch.notes.is_empty());
    assert_eq!(form.selected_profile, "test-image");
    assert_eq!(
        form.profiles.as_ref().unwrap().profiles["test-image"].image,
        "example.invalid/worker:test"
    );
    assert!(app.cloud_prototype.error.is_none());
    let revision = form.launch.revision.clone().unwrap();
    form.launch.accounts_checked = true;
    form.prices.runpod_answered();
    app.create_production_cloud(&ctx).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.cloud_prototype.production.pending_creation.is_some() {
        assert!(Instant::now() < deadline);
        app.poll_cloud_creation(&ctx);
        std::thread::yield_now();
    }
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.revision, revision);
    assert_eq!(launch.profile.image, "example.invalid/worker:test");
    let encoded = serde_json::to_vec(launch).unwrap();
    let restored: horizon_core::cloud_panel::CloudLaunch = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored.profile, launch.profile);
    assert_eq!(restored.revision, revision);
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(["cat-file", "-e", "HEAD:.horizon/cloud.yml"])
            .output()
            .unwrap()
            .status
            .code()
            .is_some_and(|code| code != 0)
    );
}

/// A repository whose commit has no settings: the dialog offers quick start without a
/// collapsed section or an error, and a click starts the public base image.
#[test]
fn quick_start_is_offered_openly_and_creates_an_image_only_cloud() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    std::fs::remove_dir_all(temp.path().join(".horizon")).unwrap();
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.repository = temp.path().to_string_lossy().into();
    app.cloud_prototype.production.title = "Quick start".into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    assert!(app.cloud_prototype.error.is_none());
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    label_position(&output, "No cloud setup yet");
    click(&ctx, &mut app, label_position(&output, "Start on the base image"));
    finish_read(&ctx, &mut app);
    let form = &mut app.cloud_prototype.production;
    assert_eq!(form.launch.configuration, Configuration::QuickStart);
    assert!(!form.launch.unconfigured);
    assert_eq!(form.selected_profile, quick_start::PROFILE);
    assert!(app.cloud_prototype.error.is_none());
    form.launch.accounts_checked = true;
    form.prices.runpod_answered();
    app.create_production_cloud(&ctx).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.cloud_prototype.production.pending_creation.is_some() {
        assert!(Instant::now() < deadline);
        app.poll_cloud_creation(&ctx);
        std::thread::yield_now();
    }
    let launch = app.cloud_prototype.groups.0.last().unwrap().remote.as_ref().unwrap();
    assert_eq!(launch.profile_name, quick_start::PROFILE);
    assert_eq!(launch.profile.image, quick_start::IMAGE);
    assert!(launch.profile.build.is_none(), "quick start never builds an image");
}

#[test]
fn quick_start_on_a_commit_with_settings_reads_those_settings_instead() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    for args in [
        vec!["add", ".horizon/cloud.yml"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Add cloud settings",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(temp.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let form = &mut app.cloud_prototype.production;
    form.repository = temp.path().to_string_lossy().into();
    form.launch.configuration = Configuration::QuickStart;
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    let form = &app.cloud_prototype.production;
    assert_eq!(form.launch.configuration, Configuration::WorkingCopy);
    assert_eq!(form.selected_profile, "test-image");
    assert!(app.cloud_prototype.error.is_none());
}

#[test]
fn quick_start_ends_with_another_repository_or_the_repository_settings() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    std::fs::remove_dir_all(temp.path().join(".horizon")).unwrap();
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.repository = temp.path().to_string_lossy().into();
    app.cloud_prototype.production.title = "Quick start".into();
    app.cloud_prototype.production.launch.configuration = Configuration::QuickStart;
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.selected_profile, quick_start::PROFILE);
    tall_click(&ctx, &mut app, "Use .horizon/cloud.yml");
    finish_read(&ctx, &mut app);
    let form = &app.cloud_prototype.production;
    assert_eq!(form.launch.configuration, Configuration::WorkingCopy);
    assert!(form.profiles.is_none() && form.launch.unconfigured);
    app.cloud_prototype.production.launch.configuration = Configuration::QuickStart;
    let other = tempfile::tempdir().unwrap();
    app.set_cloud_repository(other.path());
    assert_eq!(
        app.cloud_prototype.production.launch.configuration,
        Configuration::WorkingCopy
    );
}

#[test]
fn an_invalid_working_copy_without_committed_settings_clears_the_profile_and_placement() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    let form = &mut app.cloud_prototype.production;
    form.repository = temp.path().to_string_lossy().into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.selected_profile, "test-image");
    app.cloud_prototype.production.size = Some((8, 16));
    std::fs::write(temp.path().join(".horizon/cloud.yml"), "invalid-private-marker").unwrap();
    app.read_cloud_profiles(&ctx);
    assert!(app.cloud_prototype.production.profiles.is_none());
    finish_read(&ctx, &mut app);
    assert!(app.cloud_prototype.production.selected_profile.is_empty());
    assert!(app.cloud_prototype.production.size.is_none());
    assert!(!app.cloud_prototype.error.as_ref().unwrap().contains("private-marker"));
    app.close_cloud_creation();
    assert_eq!(
        app.cloud_prototype.production.launch.configuration,
        Configuration::WorkingCopy
    );
}

#[test]
fn rereading_committed_settings_retains_an_explicit_profile_and_valid_size() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    let yaml = format!(
        "{CONFIG}  other:\n    provider: runpod\n    image: example.invalid/other\n    cpu: 4\n    memory_gb: 8\n"
    );
    std::fs::write(temp.path().join(".horizon/cloud.yml"), yaml).unwrap();
    for args in [
        vec!["add", ".horizon/cloud.yml"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Add launch configuration",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(temp.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    app.cloud_prototype.production.repository = temp.path().to_string_lossy().into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    let form = &mut app.cloud_prototype.production;
    form.selected_profile = "other".into();
    form.size = Some((8, 16));
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    assert_eq!(app.cloud_prototype.production.selected_profile, "other");
    assert_eq!(app.cloud_prototype.production.size, Some((8, 16)));
}

/// The person edits `.horizon/cloud.yml` while New cloud is open: the size shows without a
/// button, the chosen profile stays, and an uncommitted build change is named.
#[test]
fn an_edit_of_the_working_copy_shows_while_the_dialog_is_open() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    let yaml = format!(
        "{CONFIG}  other:\n    provider: runpod\n    image: example.invalid/other\n    cpu: 4\n    memory_gb: 8\n    build:\n      context: .\n      dockerfile: Dockerfile\n"
    );
    std::fs::write(temp.path().join(".horizon/cloud.yml"), &yaml).unwrap();
    commit(temp.path());
    let form = &mut app.cloud_prototype.production;
    form.creating = true;
    form.repository = temp.path().to_string_lossy().into();
    form.title = "Edited".into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    app.cloud_prototype.production.selected_profile = "other".into();
    let cpu = |app: &HorizonApp| {
        app.cloud_prototype
            .production
            .profiles
            .as_ref()
            .map(|config| config.profiles["other"].cpu)
    };
    assert_eq!(cpu(&app), Some(4));
    std::fs::write(
        temp.path().join(".horizon/cloud.yml"),
        yaml.replace(
            "cpu: 4\n    memory_gb: 8\n    build",
            "min_cpu: 16\n    memory_gb: 8\n    build",
        )
        .replace("dockerfile: Dockerfile", "dockerfile: Other.Dockerfile"),
    )
    .unwrap();
    wait_until(&ctx, &mut app, |app| cpu(app) == Some(16));
    let form = &app.cloud_prototype.production;
    assert_eq!(form.selected_profile, "other", "the choice stays");
    assert!(
        form.profiles.is_some() && !form.launch.loading(),
        "no spinner while it reads"
    );
    assert_eq!(
        form.launch.notes,
        ["Uncommitted changes to build are not used until you commit them."]
    );
    for _ in 0..4 {
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 2400.0], None));
    }
    let shown = painted(&run_app_frame_with_input(
        &ctx,
        &mut app,
        raw_input([1400.0, 2400.0], None),
    ));
    assert!(shown.contains("Uncommitted changes to build"), "{shown}");
    assert!(!shown.contains("Read .horizon/cloud.yml"), "{shown}");
    assert!(!shown.contains("image-only"), "{shown}");
    // A commit of the same file leaves nothing to say.
    commit(temp.path());
    wait_until(&ctx, &mut app, |app| {
        app.cloud_prototype.production.launch.notes.is_empty()
    });
    assert_eq!(cpu(&app), Some(16));
}

/// A change read while the cloud is being started does not swap what the person started.
#[test]
fn an_edit_during_a_start_waits_until_the_start_ends() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    let form = &mut app.cloud_prototype.production;
    form.creating = true;
    form.repository = temp.path().to_string_lossy().into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    let cpu = |app: &HorizonApp| app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["test-image"].cpu;
    // Held by a stock watch, as by a start: both freeze what is shown.
    let profile = app.cloud_prototype.production.profiles.as_ref().unwrap().profiles["test-image"].clone();
    let selection = horizon_core::cloud_runtime::prices::watch::Selection::new(
        profile,
        horizon_core::cloud_panel::Placement {
            data_centers: vec!["EU-RO-1".into()],
            ..horizon_core::cloud_panel::Placement::default()
        },
    );
    app.cloud_prototype.production.launch.watch = Some(selection.unwrap());
    std::fs::write(
        temp.path().join(".horizon/cloud.yml"),
        CONFIG.replace("min_cpu: 4", "min_cpu: 8"),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        app.poll_cloud_launch(&ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(cpu(&app), 4);
    app.cloud_prototype.production.launch.watch = None;
    wait_until(&ctx, &mut app, |app| cpu(app) == 8);
}

fn commit(path: &std::path::Path) {
    for args in [
        vec!["add", ".horizon/cloud.yml"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "Commit cloud settings",
        ],
    ] {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
}

/// A read that failed outside the file, here a folder that is no Git checkout, can be
/// tried again: nothing in the checkout changes to start another read.
#[test]
fn a_read_that_failed_outside_the_file_can_be_tried_again() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    let form = &mut app.cloud_prototype.production;
    form.creating = true;
    form.repository = temp.path().to_string_lossy().into();
    form.title = "Retry".into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    assert!(app.cloud_prototype.error.is_some());
    assert!(!app.cloud_prototype.production.launch.unconfigured);
    tall_click(&ctx, &mut app, "Try again");
    assert!(
        app.cloud_prototype.production.launch.loading(),
        "the click starts a read"
    );
    finish_read(&ctx, &mut app);
    assert!(app.cloud_prototype.error.is_some());
}

/// An editor that saves a half-written file fails a quiet read; the choices come back with
/// the next valid save.
#[test]
fn a_half_saved_working_copy_keeps_the_choices_for_the_next_save() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    let yaml = format!(
        "{CONFIG}  other:\n    provider: runpod\n    image: example.invalid/other\n    cpu: 4\n    memory_gb: 8\n"
    );
    let path = temp.path().join(".horizon/cloud.yml");
    std::fs::write(&path, &yaml).unwrap();
    let form = &mut app.cloud_prototype.production;
    form.creating = true;
    form.repository = temp.path().to_string_lossy().into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    app.cloud_prototype.production.selected_profile = "other".into();
    std::fs::write(&path, "version: 1\ndefault: test-image\nprofiles: [").unwrap();
    wait_until(&ctx, &mut app, |app| app.cloud_prototype.production.profiles.is_none());
    assert_eq!(app.cloud_prototype.production.selected_profile, "other");
    std::fs::write(
        &path,
        yaml.replace("cpu: 4\n    memory_gb: 8\n", "cpu: 6\n    memory_gb: 8\n"),
    )
    .unwrap();
    wait_until(&ctx, &mut app, |app| app.cloud_prototype.production.profiles.is_some());
    assert_eq!(app.cloud_prototype.production.selected_profile, "other");
    assert!(app.cloud_prototype.error.is_none(), "the earlier read error goes");
}
