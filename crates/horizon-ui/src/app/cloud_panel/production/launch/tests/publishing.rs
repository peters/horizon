use super::super::super::creation::publishing as destination;
use super::*;

#[test]
fn registry_edit_invalidates_readiness_even_when_an_older_profile_read_finishes_later() {
    let (temp, mut app) = test_app();
    let ctx = Context::default();
    app.cloud_prototype.root = Some(temp.path().join("cloud"));
    apply(&mut app.cloud_prototype.production, loaded(temp.path()), false);
    app.edit_creation_registry(&ctx, Some("registry.example/team/worker".into()));
    // A late profile read may still contain readiness computed before settings were edited.
    apply(&mut app.cloud_prototype.production, loaded(temp.path()), true);
    app.cloud_prototype.production.setup.reset_form();
    app.cloud_prototype.production.launch.submitted = true;
    app.poll_cloud_launch(&ctx);
    assert!(
        app.cloud_prototype.production.setup.open,
        "Start must recheck current settings instead of trusting the old profile readiness"
    );
    assert!(app.cloud_prototype.production.pending_creation.is_none());
    assert!(app.cloud_prototype.groups.0.is_empty());
}

#[test]
fn a_queued_build_waits_for_destination_refresh_without_losing_start() {
    let (temp, mut app) = test_app();
    let ctx = Context::default();
    let mut loaded = loaded(temp.path());
    let name = loaded.prepared.config.default.clone();
    loaded.prepared.config.profiles.get_mut(&name).unwrap().build =
        Some(serde_json::from_value(serde_json::json!({"context":".","dockerfile":"Dockerfile"})).unwrap());
    let form = &mut app.cloud_prototype.production;
    apply(form, loaded, false);
    form.title = "Queued build".into();
    let key = destination::Key {
        root: temp.path().join("cloud"),
        repository: temp.path().into(),
        raw_repository: form.repository.clone(),
        profile_name: form.selected_profile.clone(),
        profile: form.profiles.as_ref().unwrap().profiles[&name].clone(),
    };
    form.publishing.sync(Some(key), &ctx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while form.publishing.busy() {
        form.publishing.poll(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    form.launch.publishing = destination::confirmation(form).unwrap();
    assert!(form.launch.publishing.is_some());
    form.launch.submitted = true;
    form.publishing.reload(&ctx);
    app.poll_cloud_launch(&ctx);
    let form = &app.cloud_prototype.production;
    assert!(form.launch.submitted, "a queued Start survives metadata reload");
    assert!(form.launch.publishing.is_some(), "the reviewed choice is retained");
    assert!(form.pending_creation.is_none());
    assert!(app.cloud_prototype.error.is_none());
}

#[test]
fn a_subdirectory_profile_load_normalizes_before_destination_review() {
    let (temp, mut app) = test_app();
    let source = temp.path().join("source");
    let nested = source.join("crates/app");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(source.join(".horizon")).unwrap();
    std::fs::write(source.join(".horizon/cloud.yml"), "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: registry.example/team/worker\n    cpu: 2\n    memory_gb: 4\n    build:\n      context: .\n      dockerfile: Dockerfile\n").unwrap();
    std::fs::write(source.join("Dockerfile"), "FROM scratch\n").unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec!["commit", "--quiet", "-m", "Add fixture"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&source)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let ctx = Context::default();
    let cloud = temp.path().join("cloud");
    app.cloud_prototype.root = Some(cloud.clone());
    app.cloud_prototype.production.repository = nested.to_string_lossy().into_owned();
    app.cloud_prototype.production.creating = true;
    app.read_cloud_profiles(&ctx);
    assert!(app.cloud_prototype.production.profiles.is_none());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.cloud_prototype.production.launch.loading_any() {
        app.poll_cloud_launch(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "profile preparation did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.cloud_prototype.error.is_none(), "{:?}", app.cloud_prototype.error);
    let form = &mut app.cloud_prototype.production;
    assert_eq!(Path::new(&form.repository), source.canonicalize().unwrap());
    assert_eq!(form.selected_profile, "cpu");
    form.publishing.sync(
        Some(destination::Key {
            root: cloud,
            repository: form.repository.clone().into(),
            raw_repository: form.repository.clone(),
            profile_name: form.selected_profile.clone(),
            profile: form.profiles.as_ref().unwrap().profiles["cpu"].clone(),
        }),
        &ctx,
    );
    while form.publishing.busy() {
        form.publishing.poll(&ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(destination::confirmation(form).unwrap().is_some());
}
