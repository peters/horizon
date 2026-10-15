use super::*;
use horizon_cloud::{Cancellation, Profile};

fn settings(root: &Path) -> Settings {
    super::super::setup::Draft::load(root).unwrap().settings
}

fn profile() -> Profile {
    serde_json::from_value(serde_json::json!({
        "image":"registry.example/team/recipe", "provider":"runpod", "cpu":2, "memory_gb":4,
        "build":{"context":".","dockerfile":"Dockerfile"}
    }))
    .unwrap()
}

#[test]
fn local_choices_are_scoped_and_legacy_profiles_keep_their_image() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut settings = settings(root.path());
    let account = Account {
        id: 42,
        login: "builder".into(),
    };
    let destination = Destination::automatic_github("builder", "git.example/team/project", "cpu").unwrap();
    let preference = Override {
        repository: repo.canonicalize().unwrap(),
        profile: "cpu".into(),
        destination: destination.repository.clone(),
        account: Some(account.clone()),
        binding: None,
    };
    settings.image_publishing.push(preference.clone());
    let cancel = Cancellation::default();
    assert_eq!(
        selection(&settings, &repo, "cpu", &profile(), &cancel).unwrap(),
        Selection::Personal { destination, account }
    );
    assert_eq!(
        selection(&settings, &repo, "gpu", &profile(), &cancel).unwrap(),
        Selection::Recipe { image: profile().image }
    );
    let encoded = serde_json::to_value(&settings).unwrap();
    assert_eq!(encoded["image_publishing"][0]["account"]["id"], 42);
    settings.image_publishing.push(preference);
    assert!(settings.validate().is_err());
}

#[test]
fn invalid_accounts_and_foreign_namespaces_cannot_be_selected() {
    let root = tempfile::tempdir().unwrap();
    let settings = settings(root.path());
    for (id, login, destination) in [
        (0, "builder", "ghcr.io/builder/worker"),
        (42, "builder", "ghcr.io/other/worker"),
        (42, "builder", "ghcr.io/builder/worker:latest"),
    ] {
        let choice = Override {
            repository: root.path().into(),
            profile: "cpu".into(),
            destination: destination.into(),
            account: Some(Account {
                id,
                login: login.into(),
            }),
            binding: None,
        };
        assert!(choice.selection(&settings).is_err());
    }
}

#[test]
fn old_settings_omit_empty_preferences_and_stale_grants_do_not_break_other_clouds() {
    let root = tempfile::tempdir().unwrap();
    let mut settings = settings(root.path());
    assert!(
        serde_json::to_value(&settings)
            .unwrap()
            .get("image_publishing")
            .is_none()
    );
    settings.image_publishing.push(Override {
        repository: root.path().into(),
        profile: "cpu".into(),
        destination: "registry.example/team/worker".into(),
        account: None,
        binding: Some("removed-grant".into()),
    });
    settings.validate().unwrap();
    assert!(settings.image_publishing[0].selection(&settings).is_err());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(selection(&settings, &root.path().join("absent"), "cpu", &profile(), &cancel).is_err());
}
