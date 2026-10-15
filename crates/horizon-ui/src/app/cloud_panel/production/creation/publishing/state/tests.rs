use super::*;

#[cfg(unix)]
fn historical_git(repository: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[cfg(unix)]
struct HistoricalFixture {
    _temp: tempfile::TempDir,
    key: Key,
}

#[cfg(unix)]
fn historical_fixture(current_name: &str) -> HistoricalFixture {
    let temp = tempfile::tempdir().unwrap();
    let repository = temp.path().join("source");
    std::fs::create_dir_all(repository.join(".horizon")).unwrap();
    let config = repository.join(".horizon/cloud.yml");
    std::fs::write(&config, "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    cpu: 2\n    memory_gb: 4\n    build:\n      context: .\n      dockerfile: Dockerfile\n").unwrap();
    std::fs::write(repository.join("Dockerfile"), "FROM scratch\n").unwrap();
    historical_git(&repository, &["init", "--quiet"]);
    historical_git(&repository, &["add", "."]);
    historical_git(
        &repository,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture: save build profile",
        ],
    );
    let revision = historical_git(&repository, &["rev-parse", "HEAD"]);
    std::fs::write(&config, format!("version: 1\ndefault: {current_name}\nprofiles:\n  {current_name}:\n    provider: runpod\n    image: public.example/current/worker\n    cpu: 2\n    memory_gb: 4\n")).unwrap();
    historical_git(&repository, &["add", "."]);
    historical_git(
        &repository,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture: replace current profile",
        ],
    );
    let cancel = Cancellation::default();
    let prepared = runtime::repository::launch::prepare_with_configuration(
        repository.to_str().unwrap(),
        &revision,
        runtime::repository::launch::Configuration::WorkingCopy,
        &runtime::command::Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        },
    )
    .unwrap();
    let root = temp.path().join("cloud");
    std::fs::create_dir(&root).unwrap();
    let mut settings = runtime::setup::load_settings(&root).unwrap();
    settings.portable_image_publishing = true;
    settings.registries = Some(
        serde_json::from_value(serde_json::json!({
            "root": root.join("registry"), "bindings": [{
                "repository":"registry.example/team/worker", "generation":"historical-fixture",
                "publish":{"username":"writer", "secret_file":root.join("absent-writer")},
                "pull":{"username":"reader", "secret_file":root.join("absent-reader")},
                "read_only_confirmed":true
            }]
        }))
        .unwrap(),
    );
    std::fs::write(root.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
    let key = Key {
        root: root.clone(),
        repository: prepared.repository,
        raw_repository: repository.to_str().unwrap().into(),
        profile_name: "cpu".into(),
        profile: prepared.config.profiles["cpu"].clone(),
    };
    HistoricalFixture { _temp: temp, key }
}

#[test]
#[cfg(unix)]
fn historical_build_choices_can_change_when_the_current_profile_is_absent_or_prebuilt() {
    for current_name in ["renamed", "cpu"] {
        let fixture = historical_fixture(current_name);
        let key = fixture.key.clone();
        let repository = key.repository.clone();
        let root = key.root.clone();
        let cancel = Cancellation::default();
        let mut state = State::default();
        state.info = Some(load(&key, &cancel).unwrap());
        state.key = Some(key);
        let selected = state.info.as_ref().unwrap().choices[0].selection.clone();
        let ctx = egui::Context::default();
        state.save(Some(selected.clone()), &ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while state.busy() {
            state.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(state.error.is_none(), "historical selection refused: {:?}", state.error);
        assert_eq!(state.selected(), Some(&selected));
        assert!(state.ready());
        state.save(None, &ctx);
        let deadline = Instant::now() + Duration::from_secs(20);
        while state.busy() {
            state.poll(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(state.error.is_none(), "historical reset refused: {:?}", state.error);
        assert!(matches!(state.selected(), Some(Selection::Automatic { .. })));
        assert!(!state.ready());
        assert_eq!(historical_git(&repository, &["status", "--porcelain"]), "");
        assert!(!root.join("absent-writer").exists());
        assert!(!root.join("registry").exists());
    }
}

fn personal() -> Selection {
    let account = runtime::github::publish::publication::Account {
        id: 17,
        login: "synthetic-user".into(),
    };
    Selection::Personal {
        destination: serde_json::from_value(serde_json::json!({
            "repository":"ghcr.io/synthetic-user/fixture",
            "publisher":{"kind":"git_hub","login":account.login}
        }))
        .unwrap(),
        account,
    }
}

fn info(pull_saved: bool) -> Info {
    Info {
        enabled: true,
        selection: Some(personal()),
        selection_error: None,
        choices: Vec::new(),
        shown_override: None,
        pull_saved,
    }
}

fn state(info: Option<Info>) -> State {
    State {
        key: Some(Key {
            root: PathBuf::from("fixture"), repository: PathBuf::from("fixture"),
            raw_repository: "fixture".into(), profile_name: "cpu".into(),
            profile: serde_json::from_value(serde_json::json!({"provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap(),
        }),
        receiver: None,
        refresh_at: None,
        cancel: Cancellation::default(),
        info,
        error: None,
        notice: None,
        prompt: None,
        changing: false,
        signing_in: None,
        awaiting_choice: false,
        setup_target: None,
        opener: None,
        open_error: None,
    }
}

fn prompt() -> Prompt {
    Prompt::Publish {
        user_code: "SYNTHETIC".into(),
        verification_uri: "https://example.invalid/device".into(),
        expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(60),
    }
}

#[test]
fn storage_edits_keep_the_owned_read_or_sign_in_but_invalidate_a_queued_profile_confirmation() {
    for signing_in in [false, true] {
        let mut state = state(Some(info(true)));
        let (sender, receiver) = channel();
        state.receiver = Some(receiver);
        if signing_in {
            state.signing_in = Some("fixture-sign-in".into());
            state.prompt = Some(prompt());
        }
        let owned = state.cancel.clone();
        let shown = super::super::Confirmation {
            scope: state.key.clone().unwrap(),
            selection: personal(),
        };
        let mut key = state.key.clone().unwrap();
        key.profile.storage.container_gb += 1;
        state.sync(Some(key.clone()), &egui::Context::default());
        assert!(
            owned.check().is_ok(),
            "storage does not change the publishing operation's inputs"
        );
        assert!(state.busy());
        assert_eq!(state.prompt.is_some(), signing_in);
        assert_eq!(state.signing_in.is_some(), signing_in);
        assert!(state.key.as_ref() == Some(&key));
        assert!(
            !state.matches_confirmation(&shown),
            "queued Start still pins the complete profile"
        );
        assert!(sender.send(Report::Loaded(Ok(info(true)))).is_ok());
    }
}

#[test]
fn personal_selection_waits_for_separate_pull_access_and_current_preparation() {
    let mut state = state(Some(info(false)));
    assert!(!state.ready());
    assert!(state.snapshot().is_none());
    state.info.as_mut().unwrap().pull_saved = true;
    assert!(state.ready());
    assert!(state.snapshot().is_some_and(|proof| proof.selection == personal()));
    let (_tx, rx) = channel();
    state.receiver = Some(rx);
    assert!(!state.ready());
    assert!(state.snapshot().is_none());
}

#[test]
fn coalesced_prompt_completion_ends_the_prompt_and_the_job_in_one_poll() {
    let mut state = state(Some(info(true)));
    let (tx, rx) = channel();
    state.receiver = Some(rx);
    state.signing_in = Some("synthetic-operation".into());
    tx.send(Report::Prompt(prompt())).unwrap();
    tx.send(Report::Prompt(Prompt::Published { allowed: false })).unwrap();
    tx.send(Report::Finished(Err("Sign-in cancelled".into()))).unwrap();
    state.poll(&egui::Context::default());
    assert!(!state.busy());
    assert!(state.prompt.is_none());
    assert!(state.signing_in.is_none());
    assert_eq!(state.error.as_deref(), Some("Sign-in cancelled"));
    assert!(!state.ready());
}

#[test]
fn a_disconnected_preparation_clears_its_device_code_and_blocks_start() {
    let mut state = state(Some(info(true)));
    let (tx, rx) = channel();
    state.receiver = Some(rx);
    state.prompt = Some(prompt());
    state.signing_in = Some("synthetic-operation".into());
    drop(tx);
    state.poll(&egui::Context::default());
    assert!(!state.busy());
    assert!(state.prompt.is_none());
    assert!(state.signing_in.is_none());
    assert!(state.error.is_some());
    assert!(!state.ready());
}

#[test]
fn changing_scope_cancels_and_discards_an_old_loaded_answer() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repo");
    std::fs::create_dir(&repository).unwrap();
    let profile=serde_json::from_value(serde_json::json!({"provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap();
    let key = Key {
        root: root.path().to_owned(),
        raw_repository: repository.to_str().unwrap().into(),
        repository,
        profile_name: "cpu".into(),
        profile,
    };
    let mut state = state(Some(info(true)));
    state.key = Some(key.clone());
    let old_cancel = state.cancel.clone();
    let (tx, rx) = channel();
    state.receiver = Some(rx);
    tx.send(Report::Loaded(Ok(info(true)))).unwrap();
    state.sync(
        Some(Key {
            profile_name: "gpu".into(),
            ..key
        }),
        &egui::Context::default(),
    );
    assert!(old_cancel.check().is_err());
    assert_eq!(state.key.as_ref().unwrap().profile_name, "gpu");
    assert!(!matches!(state.selected(), Some(Selection::Personal { .. })));
    assert!(tx.send(Report::Loaded(Ok(info(true)))).is_err());
}

#[test]
fn a_reload_keeps_the_destination_visible_but_cannot_confirm_stale_metadata() {
    let root = tempfile::tempdir().unwrap();
    let mut state = state(Some(info(true)));
    state.key = Some(Key {
        root: root.path().to_owned(),
        repository: root.path().to_owned(),
        raw_repository: root.path().to_str().unwrap().into(),
        profile_name: "cpu".into(),
        profile: serde_json::from_value(serde_json::json!({
            "provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4
        }))
        .unwrap(),
    });
    state.notice = Some("Destination changed".into());
    state.reload(&egui::Context::default());
    assert_eq!(
        state.notice.as_deref(),
        Some("Destination changed"),
        "refresh preserves why the watch stopped"
    );
    assert_eq!(state.selected(), Some(&personal()));
    assert!(state.busy());
    assert!(!state.ready());
    assert!(state.snapshot().is_none());
}

#[test]
fn a_disabled_feature_keeps_the_explicit_recipe_without_reading_git_or_sign_in() {
    let root = tempfile::tempdir().unwrap();
    let key = Key {
        root: root.path().to_owned(),
        repository: root.path().to_owned(),
        raw_repository: root.path().to_str().unwrap().into(),
        profile_name: "cpu".into(),
        profile: serde_json::from_value(serde_json::json!({
            "provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4,
            "build":{"context":".","dockerfile":"Dockerfile"}
        }))
        .unwrap(),
    };
    let loaded = load(&key, &Cancellation::default()).unwrap();
    assert!(!loaded.enabled);
    assert_eq!(
        loaded.selection,
        Some(Selection::Recipe {
            image: key.profile.image.clone()
        })
    );
    assert!(state(Some(loaded)).ready());
    let mut automatic = key;
    automatic.profile.image.clear();
    let loaded = load(&automatic, &Cancellation::default()).unwrap();
    assert!(loaded.selection.is_none());
    assert!(!state(Some(loaded)).ready());
}

#[test]
fn cancelling_sign_in_only_ends_its_owned_attempt_and_allows_a_new_attempt() {
    let mut state = state(Some(info(true)));
    state.signing_in = Some("fixture-sign-in".into());
    let owned = state.cancel.clone();
    let unrelated = Cancellation::default();
    state.skip_sign_in();
    assert!(owned.check().is_err(), "the in-flight authentication is cancelled");
    assert!(unrelated.check().is_ok(), "another operation remains live");
    state.reset_operation();
    assert!(owned.check().is_err(), "the cancelled attempt stays cancelled");
    assert!(state.cancel.check().is_ok(), "a new attempt can run");
    state.skip_sign_in();
    assert!(state.cancel.check().is_ok(), "no active sign-in is not cancelled");
}

#[test]
fn expired_metadata_blocks_start_until_shared_policy_has_been_read_again() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().canonicalize().unwrap();
    let mut state = state(Some(info(true)));
    state.key = Some(Key {
        root: repository.clone(),
        repository: repository.clone(),
        raw_repository: repository.to_str().unwrap().into(),
        profile_name: "cpu".into(),
        profile: serde_json::from_value(serde_json::json!({
            "provider":"runpod","image":"registry.example/team/worker","cpu":2,"memory_gb":4,
            "build":{"context":".","dockerfile":"Dockerfile"}
        }))
        .unwrap(),
    });
    // The cached enabled publisher has been replaced by default, disabled settings.
    state.refresh_at = Some(Instant::now());
    assert!(!state.ready());
    assert!(state.snapshot().is_none());
    let ctx = egui::Context::default();
    state.poll(&ctx);
    assert!(state.busy());
    assert_eq!(
        state.selected(),
        Some(&personal()),
        "keep the displayed target during reload"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.busy() {
        state.poll(&ctx);
        assert!(Instant::now() < deadline, "shared metadata read settles");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        state.selected(),
        Some(&Selection::Recipe {
            image: "registry.example/team/worker".into()
        })
    );
    assert!(!state.info.as_ref().unwrap().enabled);
    assert!(state.ready(), "the fresh explicit recipe uses the disabled policy");
    assert!(!root.path().join("settings.json").exists());
    assert!(!root.path().join("registry").exists());
}

#[test]
fn an_owned_sign_in_blocks_start_before_and_after_its_prompt_arrives() {
    let state = state(Some(info(true)));
    let profiles = horizon_core::cloud_panel::CloudConfig::parse("version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    image: registry.example/team/worker\n    cpu: 2\n    memory_gb: 4\n    build:\n      context: .\n      dockerfile: Dockerfile\n").unwrap();
    let mut form = crate::app::cloud_panel::production::Production {
        repository: "fixture".into(),
        selected_profile: "cpu".into(),
        profiles: Some(profiles),
        publishing: state,
        ..Default::default()
    };
    let (_tx, rx) = channel();
    form.publishing.receiver = Some(rx);
    assert!(
        super::super::start_blocked(&form).is_none(),
        "the shown same-scope choice can queue during a read"
    );
    for prompt_arrived in [false, true] {
        form.publishing.signing_in = Some("owned-attempt".into());
        form.publishing.prompt = prompt_arrived.then(prompt);
        assert!(super::super::start_blocked(&form).is_some());
        assert!(super::super::start_confirmation(&form).is_err());
        super::super::super::selector::summary::start(&mut form);
        assert!(
            !form.launch.submitted,
            "Start must not disable the owned sign-in controls"
        );
        assert!(form.publishing.reviewed_snapshot().is_none());
    }
}

#[test]
fn opening_sign_in_is_off_thread_bounded_and_keeps_metadata_errors_separate() {
    let mut state = state(Some(info(true)));
    state.error = Some("Keep the metadata error".into());
    let ui_thread = std::thread::current().id();
    let (release, gate) = channel();
    let ctx = egui::Context::default();
    state.open_with("https://example.invalid/device".into(), &ctx, move |url| {
        assert_ne!(std::thread::current().id(), ui_thread);
        assert_eq!(url, "https://example.invalid/device");
        gate.recv_timeout(Duration::from_secs(5)).unwrap();
        Err(std::io::Error::other("synthetic opener failure"))
    });
    assert!(state.opening_url(), "opening must return before the worker is released");
    state.open_with("https://example.invalid/duplicate".into(), &ctx, |_| {
        panic!("an active opener must not start another process")
    });
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.opening_url() {
        state.poll_opener();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        state
            .open_error
            .as_deref()
            .unwrap()
            .contains("synthetic opener failure")
    );
    assert_eq!(state.error.as_deref(), Some("Keep the metadata error"));
}

#[test]
fn changing_the_saved_destination_blocks_new_intent_until_it_settles() {
    let root = tempfile::tempdir().unwrap();
    let mut state = state(Some(info(true)));
    state.key.as_mut().unwrap().root = root.path().join("cloud");
    state.key.as_mut().unwrap().repository = root.path().join("missing-checkout");
    assert!(state.reviewable());
    state.save(None, &egui::Context::default());
    assert!(state.busy());
    assert!(
        !state.reviewable(),
        "a pending save cannot queue the previous destination"
    );
    assert!(state.reviewed_snapshot().is_none());
}

#[test]
fn a_failed_post_change_read_cannot_queue_the_old_choice_while_retrying() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = state(Some(info(true)));
    state.key.as_mut().unwrap().root = temp.path().join("cloud");
    state.key.as_mut().unwrap().repository = temp.path().into();
    state.awaiting_choice = true;
    let (tx, rx) = channel();
    state.receiver = Some(rx);
    tx.send(Report::Loaded(Err(
        "Synthetic read failure after changing the destination".into(),
    )))
    .unwrap();
    state.poll(&egui::Context::default());
    assert!(state.error.is_some());
    state.reload(&egui::Context::default());
    assert!(state.error.is_none());
    assert!(state.busy());
    assert!(
        !state.reviewable(),
        "retrying an unread changed destination cannot queue the old proof"
    );
    assert!(state.reviewed_snapshot().is_none());
}
