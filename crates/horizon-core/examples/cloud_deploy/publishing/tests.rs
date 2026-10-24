use super::*;
use horizon_core::cloud_runtime::{
    github::publish::publication::Account,
    publishing::{Override, Publication},
    setup::Draft,
    state::Deployment,
};
use std::process::Command;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Settings, Profile) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    let settings = Draft::load(root.path()).unwrap().settings;
    let profile = serde_json::from_value(serde_json::json!({"provider":"runpod", "cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}})).unwrap();
    (root, repo.canonicalize().unwrap(), settings, profile)
}

fn origin(repo: &Path, url: &str) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["config", "remote.origin.url", url])
            .status()
            .unwrap()
            .success()
    );
}

fn cached_account(settings: &Settings) {
    std::fs::create_dir_all(&settings.docker_config).unwrap();
    let path = settings.docker_config.join("horizon-github-packages.json");
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({"login":"builder","access_token":"synthetic-access","refresh_token":"synthetic-refresh","access_expires_at":4_000_000_000_u64,"refresh_expires_at":4_000_000_000_u64})).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn changing_the_source_destination_invalidates_the_confirmed_plan() {
    let (root, repo, settings, profile) = fixture();
    cached_account(&settings);
    let state_root = root.path().join("not-created");
    let cancel = Cancellation::default();
    origin(&repo, "https://git.example/team/first.git");
    let shown = selection(&repo, "cpu", &profile, &settings, &state_root, "plan-cloud", &cancel).unwrap();
    assert!(shown.repository().unwrap().starts_with("ghcr.io/builder/"));
    let token = super::super::deploy_token(&profile, &settings, None, &shown);
    origin(&repo, "https://git.example/team/second.git");
    let changed = selection(&repo, "cpu", &profile, &settings, &state_root, "plan-cloud", &cancel).unwrap();
    assert_ne!(shown.repository(), changed.repository());
    assert!(super::super::confirm(&profile, &settings, &changed, None, Some(&token)).is_err());
    assert!(!state_root.exists(), "planning does not create state or locks");
}

#[test]
fn unrelated_local_preferences_do_not_invalidate_a_plan() {
    let (_root, repo, mut settings, profile) = fixture();
    let selected = Selection::Recipe {
        image: "registry.example/team/worker".into(),
    };
    let token = super::super::deploy_token(&profile, &settings, None, &selected);
    settings.image_publishing.push(Override {
        repository: repo,
        profile: "another-profile".into(),
        destination: "ghcr.io/builder/another".into(),
        account: Some(Account {
            id: 42,
            login: "builder".into(),
        }),
        binding: None,
    });
    assert_eq!(super::super::deploy_token(&profile, &settings, None, &selected), token);
}

#[test]
fn a_retry_plan_keeps_the_recorded_destination_after_local_defaults_change() {
    let (root, repo, settings, mut profile) = fixture();
    let account = Account {
        id: 42,
        login: "builder".into(),
    };
    let destination =
        horizon_cloud::image_publishing::Destination::automatic_github("builder", "git.example/team/original", "cpu")
            .unwrap();
    profile.image.clone_from(&destination.repository);
    let record = Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination: destination.clone(),
        account: Some(account.clone()),
    };
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"retained","repository":repo,"revision":"a".repeat(40),"profile":profile,
        "publication":record,"stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let state_root = root.path().join("cloud");
    {
        let store = Store::lock(&state_root).unwrap();
        store.save(&state).unwrap();
    }
    profile.image.clear();
    origin(&repo, "https://another.example/another/source.git");
    assert_eq!(
        selection(
            &repo,
            "cpu",
            &profile,
            &settings,
            &state_root,
            "retained",
            &Cancellation::default()
        )
        .unwrap(),
        Selection::Personal { destination, account }
    );
    assert!(
        selection(
            &repo,
            "cpu",
            &profile,
            &settings,
            &state_root,
            "another-cloud",
            &Cancellation::default()
        )
        .is_err()
    );
}

#[test]
fn a_legacy_retry_plan_ignores_later_machine_preferences() {
    let (root, repo, mut settings, mut profile) = fixture();
    profile.image = "registry.example/team/original".into();
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":1,"cloud_id":"legacy","repository":repo,"revision":"a".repeat(40),"profile":profile,
        "stage":"Validate","operation":{"state":"prepared"},"spec":null,"worker":null,"sessions":[]
    }))
    .unwrap();
    let state_root = root.path().join("cloud");
    Store::lock(&state_root).unwrap().save(&state).unwrap();
    settings.image_publishing.push(Override {
        repository: repo.clone(),
        profile: "cpu".into(),
        destination: "ghcr.io/builder/later-choice".into(),
        account: Some(Account {
            id: 42,
            login: "builder".into(),
        }),
        binding: None,
    });
    profile.image.clear();
    assert_eq!(
        selection(
            &repo,
            "cpu",
            &profile,
            &settings,
            &state_root,
            "legacy",
            &Cancellation::default()
        )
        .unwrap(),
        Selection::Recipe {
            image: state.profile.image
        }
    );
}

#[test]
#[cfg(unix)]
fn an_existing_image_can_be_planned_without_a_publishing_grant_but_cannot_be_rebuilt_by_prepare_image() {
    let (root, repo, settings, profile) = fixture();
    let destination = horizon_cloud::image_publishing::Destination::bound(
        "registry.example/team/worker".into(),
        "synthetic-grant".into(),
    )
    .unwrap();
    let mut retained = profile.clone();
    retained.image.clone_from(&destination.repository);
    let record = Publication {
        profile: "cpu".into(),
        requested_image: String::new(),
        destination: destination.clone(),
        account: None,
    };
    let state: Deployment = serde_json::from_value(serde_json::json!({
        "version":5,"cloud_id":"retained","repository":repo,"revision":"a".repeat(40),"profile":retained,
        "publication":record,"stage":"Validate","operation":{"state":"prepared"},"worker":null,"sessions":[],
        "spec":{"operation_id":"retained","profile":retained,
            "image_digest":format!("registry.example/team/worker@sha256:{}","a".repeat(64)),
            "public_key":"unused","registry_auth_id":null,"gpu_types":[],"cpu_flavors":[],"data_centers":[]}
    }))
    .unwrap();
    let state_root = root.path().join("cloud");
    Store::lock(&state_root).unwrap().save(&state).unwrap();
    let before = std::fs::read(state_root.join("deployment.json")).unwrap();
    let cancel = Cancellation::default();
    assert_eq!(
        selection(&repo, "cpu", &profile, &settings, &state_root, "retained", &cancel).unwrap(),
        Selection::Registry { destination }
    );
    let mut changed_profile = profile;
    changed_profile.cpu *= 2;
    let request = horizon_core::cloud_runtime::deployment::Request::new(
        "retained".into(),
        repo,
        "a".repeat(40),
        changed_profile,
        state_root.clone(),
        settings,
    );
    let error = super::super::prepare_image(&request, &cancel, &|_| {}).unwrap_err();
    assert!(matches!(error, Error::Invalid(message) if message.contains("already has a prepared worker image")));
    assert_eq!(std::fs::read(state_root.join("deployment.json")).unwrap(), before);
}

#[cfg(unix)]
#[path = "tests/rebuild.rs"]
mod rebuild;

// Durable image-preparation records are Unix-only.
#[cfg(unix)]
#[path = "tests/historical.rs"]
mod historical;
