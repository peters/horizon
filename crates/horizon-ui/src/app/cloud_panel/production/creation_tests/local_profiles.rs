use super::*;
use horizon_core::cloud_runtime::repository::launch::Configuration;

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

#[test]
fn local_choice_loads_untracked_settings_and_captures_the_profile_for_creation() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    app.cloud_prototype.production.creating = true;
    app.cloud_prototype.production.repository = temp.path().to_string_lossy().into();
    app.cloud_prototype.production.title = "Test image".into();
    app.read_cloud_profiles(&ctx);
    finish_read(&ctx, &mut app);
    assert!(app.cloud_prototype.production.profiles.is_none());
    assert!(app.cloud_prototype.error.as_ref().unwrap().contains("no readable"));
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "Use local image-only settings"));
    finish_read(&ctx, &mut app);
    let form = &mut app.cloud_prototype.production;
    assert_eq!(form.launch.configuration, Configuration::LocalImageOnly);
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

#[test]
fn quick_start_offers_the_builtin_profile_for_a_repository_without_settings() {
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
    assert!(app.cloud_prototype.error.as_ref().unwrap().contains("quick start"));
    for _ in 0..4 {
        frame(&ctx, &mut app, Vec::new(), Modifiers::NONE);
    }
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(&ctx, &mut app, label_position(&output, "No cloud configuration yet?"));
    let output = run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    click(
        &ctx,
        &mut app,
        label_position(&output, "Quick start on the public base image"),
    );
    finish_read(&ctx, &mut app);
    let form = &app.cloud_prototype.production;
    assert_eq!(form.launch.configuration, Configuration::QuickStart);
    assert_eq!(form.selected_profile, "quick-start");
    let profile = &form.profiles.as_ref().unwrap().profiles["quick-start"];
    assert_eq!(
        profile.image,
        horizon_core::cloud_runtime::repository::launch::quick_start::IMAGE
    );
    assert!(profile.build.is_none());
    assert!(app.cloud_prototype.error.is_none());
}

#[test]
fn invalid_local_reload_clears_the_previous_profile_and_placement() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    fixture(temp.path());
    let form = &mut app.cloud_prototype.production;
    form.repository = temp.path().to_string_lossy().into();
    form.launch.configuration = Configuration::LocalImageOnly;
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
        Configuration::Committed
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
