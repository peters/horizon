use super::*;
use crate::cloud_runtime::{
    publishing,
    registry::{Auth, Binding, Config},
};
use horizon_cloud::{Profile, image_publishing::Destination};

fn fixture(root: &Path) -> (Settings, Profile) {
    let mut settings = super::super::defaults(root);
    settings.registries = Some(Config {
        root: root.join("registry"),
        bindings: vec![Binding {
            repository: "registry.example/team/worker".into(),
            generation: "synthetic-generation".into(),
            publish: Some(Auth {
                username: "builder".into(),
                secret_file: root.join("push"),
                expires_at: None,
            }),
            pull: Auth {
                username: "reader".into(),
                secret_file: root.join("pull"),
                expires_at: None,
            },
            read_only_confirmed: true,
            retired: Vec::new(),
        }],
    });
    let profile = serde_json::from_value(serde_json::json!({"image":"registry.example/team/recipe","provider":"runpod","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap();
    (settings, profile)
}

#[test]
fn saving_and_restoring_one_choice_preserves_other_machine_settings() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let (mut settings, profile) = fixture(root.path());
    settings.image_publishing.push(Override {
        repository: repo.canonicalize().unwrap(),
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(publishing::grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    });
    let choice = publishing::selection(&settings, &repo, "cpu", &profile, &Cancellation::default()).unwrap();
    settings.image_publishing.clear();
    settings.image_publishing.push(Override {
        repository: root.path().join("another-repo"),
        profile: "gpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some(publishing::grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap()),
    });
    settings.cpu_flavors = vec!["different-machine-preference".into()];
    let path = root.path().join("settings.json");
    std::fs::write(&path, storage::encode(&settings).unwrap()).unwrap();
    let cancel = Cancellation::default();
    let saved = save_image_destination(root.path(), &repo, "cpu", None, Some(&choice), &cancel).unwrap();
    assert_eq!(saved.after, std::fs::read(&path).unwrap());
    let observed = Settings::load(&path).unwrap();
    assert_eq!(observed.cpu_flavors, settings.cpu_flavors);
    assert_eq!(observed.image_publishing.len(), 2);
    assert_eq!(
        publishing::selection(&observed, &repo, "cpu", &profile, &Cancellation::default()).unwrap(),
        choice
    );
    assert!(save_image_destination(root.path(), &repo, "cpu", None, None, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), saved.after);
    save_image_destination(
        root.path(),
        &repo,
        "cpu",
        Some(&observed.image_publishing[1]),
        None,
        &cancel,
    )
    .unwrap();
    assert_eq!(
        Settings::load(&path).unwrap().image_publishing,
        settings.image_publishing
    );
    assert!(
        !repo.join(".horizon").exists(),
        "the preference does not enter repository files"
    );
}

#[test]
fn a_changed_publishing_grant_is_refused_without_replacing_settings() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let (mut settings, _) = fixture(root.path());
    let choice = Selection::Registry {
        destination: Destination::bound(
            "registry.example/team/worker".into(),
            publishing::grant_id(&settings.registries.as_ref().unwrap().bindings[0]).unwrap(),
        )
        .unwrap(),
    };
    settings.registries.as_mut().unwrap().bindings[0]
        .publish
        .as_mut()
        .unwrap()
        .secret_file = root.path().join("another-account");
    let path = root.path().join("settings.json");
    let before = storage::encode(&settings).unwrap();
    std::fs::write(&path, &before).unwrap();
    assert!(save_image_destination(root.path(), &repo, "cpu", None, Some(&choice), &Cancellation::default()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn cancellation_prevents_even_the_settings_lock_from_being_created() {
    let root = tempfile::tempdir().unwrap();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(save_image_destination(&root.path().join("absent"), root.path(), "cpu", None, None, &cancel).is_err());
    assert!(!root.path().join("absent").exists());
}

#[test]
fn a_first_personal_preference_and_default_restore_need_no_existing_settings_file() {
    for restore in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let account = crate::cloud_runtime::github::publish::publication::Account {
            id: 42,
            login: "builder".into(),
        };
        let choice = Selection::Personal {
            destination: Destination::automatic_github("builder", "https://git.example/team/project", "cpu").unwrap(),
            account,
        };
        let committed = save_image_destination(
            root.path(),
            &repo,
            "cpu",
            None,
            (!restore).then_some(&choice),
            &Cancellation::default(),
        )
        .unwrap();
        assert!(committed.before.is_none());
        assert_eq!(
            std::fs::read(root.path().join("settings.json")).unwrap(),
            committed.after
        );
        let saved = Settings::load(&root.path().join("settings.json")).unwrap();
        assert_eq!(saved.image_publishing.len(), usize::from(!restore));
        if !restore {
            assert_eq!(saved.image_publishing[0].selection(&saved).unwrap(), choice);
        }
        assert!(
            !root.path().join("docker").exists(),
            "saving metadata reads or writes no authentication chain"
        );
    }
}

#[test]
fn settings_within_the_source_are_refused_before_a_lock_or_write() {
    let root = tempfile::tempdir().unwrap();
    let (settings, _) = fixture(root.path());
    let path = root.path().join("settings.json");
    let before = storage::encode(&settings).unwrap();
    std::fs::write(&path, &before).unwrap();
    assert!(save_image_destination(root.path(), root.path(), "cpu", None, None, &Cancellation::default()).is_err());
    assert!(!root.path().join("settings.lock").exists());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn pull_rotation_keeps_the_shown_publication_grant_without_reading_secrets() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let (mut settings, _) = fixture(root.path());
    let selected = publishing::choices(&settings).unwrap().remove(0);
    let binding = &mut settings.registries.as_mut().unwrap().bindings[0];
    binding.generation = "rotated-pull".into();
    binding.pull.secret_file = root.path().join("rotated-pull");
    let path = root.path().join("settings.json");
    std::fs::write(&path, storage::encode(&settings).unwrap()).unwrap();
    save_image_destination(
        root.path(),
        &repo,
        "cpu",
        None,
        Some(&selected),
        &Cancellation::default(),
    )
    .unwrap();
    let mut saved = Settings::load(&path).unwrap();
    assert_eq!(saved.image_publishing[0].selection(&saved).unwrap(), selected);
    assert!(!root.path().join("push").exists());
    assert!(!root.path().join("rotated-pull").exists());
    saved.registries.as_mut().unwrap().bindings[0]
        .publish
        .as_mut()
        .unwrap()
        .secret_file = root.path().join("other-publisher");
    std::fs::write(&path, storage::encode(&saved).unwrap()).unwrap();
    let reloaded = Settings::load(&path).unwrap();
    assert!(
        reloaded.image_publishing[0].selection(&reloaded).is_err(),
        "reload cannot redirect the saved grant"
    );
}

#[test]
fn another_account_at_the_same_uri_is_not_overwritten_by_a_stale_window() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let (mut settings, _) = fixture(root.path());
    let shown = Override {
        repository: repo.canonicalize().unwrap(),
        profile: "cpu".into(),
        destination: "ghcr.io/builder/worker".into(),
        account: Some(crate::cloud_runtime::github::publish::publication::Account {
            id: 42,
            login: "builder".into(),
        }),
        binding: None,
    };
    let mut other = shown.clone();
    other.account.as_mut().unwrap().id = 43;
    settings.image_publishing.push(other);
    let before = storage::encode(&settings).unwrap();
    let path = root.path().join("settings.json");
    std::fs::write(&path, &before).unwrap();
    assert!(save_image_destination(root.path(), &repo, "cpu", Some(&shown), None, &Cancellation::default()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let selected = settings.image_publishing[0].selection(&settings).unwrap();
    save_image_destination(
        root.path(),
        &repo,
        "cpu",
        Some(&settings.image_publishing[0]),
        Some(&selected),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(
        Settings::load(&path).unwrap().image_publishing[0]
            .account
            .as_ref()
            .unwrap()
            .id,
        43
    );
}
