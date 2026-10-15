use super::*;
use std::process::Command;

struct Fixture {
    _root: tempfile::TempDir,
    cloud: PathBuf,
    request: Request,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("source");
    let cloud = root.path().join("cloud");
    std::fs::create_dir_all(repository.join(".horizon")).unwrap();
    std::fs::create_dir(&cloud).unwrap();
    std::fs::write(repository.join(".horizon/cloud.yml"), "version: 1\ndefault: cpu\nprofiles:\n  cpu:\n    provider: runpod\n    cpu: 2\n    memory_gb: 4\n    build:\n      context: .\n      dockerfile: Dockerfile\n").unwrap();
    std::fs::write(repository.join("Dockerfile"), "FROM scratch\n").unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
        vec![
            "remote",
            "add",
            "origin",
            "https://gitlab.example/group/nested/source.git",
        ],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(&repository)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let mut settings = setup::load_settings(&cloud).unwrap();
    settings.portable_image_publishing = true;
    settings.registries = Some(
        serde_json::from_value(serde_json::json!({"root":cloud.join("registry"),"bindings":[{
            "repository":"registry.example/group/worker","generation":"synthetic-generation",
            "publish":{"username":"builder","secret_file":cloud.join("absent-publish"),"expires_at":null},
            "pull":{"username":"reader","secret_file":cloud.join("absent-pull"),"expires_at":null},
            "read_only_confirmed":true,"retired":[]
        }]}))
        .unwrap(),
    );
    std::fs::write(cloud.join("settings.json"), serde_json::to_vec(&settings).unwrap()).unwrap();
    Fixture {
        _root: root,
        cloud,
        request: Request {
            repository,
            profile: "cpu".into(),
            action: Action::List {},
        },
    }
}

fn run(fixture: &Fixture) -> Result<Info> {
    manage(
        &fixture.cloud,
        &fixture.request,
        "synthetic-operation",
        &Runner {
            cancel: &Cancellation::default(),
            emit: &|_| {},
            secrets: Vec::new(),
        },
    )
}

#[test]
fn list_is_nonmutating_and_reads_no_registry_secret_or_compute_key() {
    let fixture = fixture();
    let before = std::fs::read(fixture.cloud.join("settings.json")).unwrap();
    let result = run(&fixture).unwrap();
    assert!(matches!(
        result.selection,
        Some(Selection::Automatic { login: None, .. })
    ));
    assert_eq!(result.choices.len(), 1);
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("absent-publish"));
    assert!(!encoded.contains("absent-pull"));
    assert_eq!(std::fs::read(fixture.cloud.join("settings.json")).unwrap(), before);
    assert!(!fixture.cloud.join("registry").exists());
}

#[test]
#[cfg_attr(target_os = "windows", ignore = "Cloud settings require Unix directory durability")]
fn selection_and_reset_share_canonical_scope_and_reject_stale_confirmation() {
    let mut fixture = fixture();
    let choice = run(&fixture).unwrap().choices.remove(0).selection;
    std::fs::create_dir(fixture.request.repository.join("nested")).unwrap();
    fixture.request.repository.push("nested");
    fixture.request.action = Action::Select {
        shown: None,
        destination: choice.clone(),
    };
    let selected = run(&fixture).unwrap();
    assert_eq!(selected.selection, Some(choice));
    let before = std::fs::read(fixture.cloud.join("settings.json")).unwrap();
    fixture.request.action = Action::Reset { shown: None };
    assert!(run(&fixture).is_err());
    assert_eq!(std::fs::read(fixture.cloud.join("settings.json")).unwrap(), before);
    fixture.request.action = Action::Reset {
        shown: selected.shown_override,
    };
    assert!(matches!(
        run(&fixture).unwrap().selection,
        Some(Selection::Automatic { .. })
    ));
    assert!(!fixture.cloud.join("docker").exists());
}

#[test]
fn invalid_auth_scope_or_stale_choice_never_opens_a_signin_store() {
    let mut fixture = fixture();
    fixture.request.profile = "missing".into();
    fixture.request.action = Action::Authenticate {
        shown: None,
        switch: true,
    };
    assert!(run(&fixture).is_err());
    assert!(!fixture.cloud.join("docker").exists());
    fixture.request.profile = "cpu".into();
    fixture.request.action = Action::Authenticate {
        shown: Some(Override {
            repository: fixture.request.repository.clone(),
            profile: "cpu".into(),
            destination: "registry.example/group/worker".into(),
            account: None,
            binding: Some("old".into()),
        }),
        switch: true,
    };
    assert!(run(&fixture).unwrap_err().to_string().contains("changed"));
    assert!(!fixture.cloud.join("docker").exists());
}

#[test]
fn a_caller_cannot_substitute_an_arbitrary_personal_package_for_authentication() {
    let mut fixture = fixture();
    let before = std::fs::read(fixture.cloud.join("settings.json")).unwrap();
    fixture.request.action = Action::Select {
        shown: None,
        destination: Selection::Personal {
            destination: horizon_cloud::image_publishing::Destination {
                repository: "ghcr.io/builder/existing-package".into(),
                publisher: horizon_cloud::image_publishing::Publisher::GitHub {
                    login: "builder".into(),
                },
            },
            account: crate::cloud_runtime::github::publish::publication::Account {
                id: 42,
                login: "builder".into(),
            },
        },
    };
    assert!(run(&fixture).unwrap_err().to_string().contains("use authenticate"));
    assert_eq!(std::fs::read(fixture.cloud.join("settings.json")).unwrap(), before);
    assert!(!fixture.cloud.join("docker").exists());
}

#[test]
#[ignore = "set HORIZON_TEST_REPOSITORY and HORIZON_TEST_PROFILE for a read-only real checkout check"]
fn real_repository_destination_listing_preserves_source_and_machine_settings() {
    let mut fixture = fixture();
    fixture.request.repository = PathBuf::from(std::env::var_os("HORIZON_TEST_REPOSITORY").unwrap());
    fixture.request.profile = std::env::var("HORIZON_TEST_PROFILE").unwrap();
    let source = fixture.request.repository.join(".horizon/cloud.yml");
    let source_before = std::fs::read(&source).unwrap();
    let settings = fixture.cloud.join("settings.json");
    let settings_before = std::fs::read(&settings).unwrap();
    let result = run(&fixture).unwrap();
    assert!(result.enabled);
    assert_eq!(result.choices.len(), 1);
    assert_eq!(std::fs::read(source).unwrap(), source_before);
    assert_eq!(std::fs::read(settings).unwrap(), settings_before);
    assert!(!fixture.cloud.join("docker").exists());
    assert!(!fixture.cloud.join("registry").exists());
}

#[test]
fn destination_authentication_cannot_share_a_cloud_or_another_attempts_skip_key() {
    let mut fixture = fixture();
    let cloud_id = "a-live-cloud";
    let cancel = Cancellation::default();
    let runner = Runner {
        cancel: &cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let mut keys = Vec::new();
    for switch in [false, true] {
        fixture.request.action = Action::Authenticate { shown: None, switch };
        let result = manage_with(
            &fixture.cloud,
            &fixture.request,
            cloud_id,
            None,
            &runner,
            |_, key, _| {
                keys.push(key.to_owned());
                Err(Error::Invalid("synthetic stop before sign-in"))
            },
        );
        assert!(matches!(result, Err(Error::Invalid("synthetic stop before sign-in"))));
    }
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|key| key != cloud_id && !horizon_cloud::valid_id(key)));
    assert_ne!(keys[0], keys[1]);
    assert!(!fixture.cloud.join("docker").exists());
}

#[test]
fn a_receipt_cannot_widen_its_owned_repository_scope() {
    let mut fixture = fixture();
    let destination = run(&fixture).unwrap().choices[0].selection.clone();
    let before = std::fs::read(fixture.cloud.join("settings.json")).unwrap();
    let directory = fixture.request.repository.join("subdirectory");
    std::fs::create_dir(&directory).unwrap();
    fixture.request.repository = directory.clone();
    fixture.request.action = Action::Select {
        shown: None,
        destination,
    };
    let owner = super::super::operations::Owner {
        actor: "fixture-agent".into(),
        host: "fixture-host".into(),
        root: fixture.cloud.clone(),
        repository: directory,
    };
    let mut manager = super::super::operations::Manager::default();
    manager
        .start(
            owner.clone(),
            fixture.request.clone(),
            "scope-probe",
            std::sync::Arc::new(|| {}),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let status = manager.status(&owner, "scope-probe").unwrap();
        if status.done {
            assert!(
                status
                    .error
                    .as_deref()
                    .is_some_and(|message| message.contains("Git root"))
            );
            assert!(status.result.is_none());
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(fixture.cloud.join("settings.json")).unwrap(), before);
    assert!(!fixture.cloud.join("registry").exists());
}

#[test]
fn cancellation_during_metadata_selection_is_not_a_successful_listing() {
    let fixture = fixture();
    let profile: Profile = serde_json::from_value(serde_json::json!({
        "provider":"runpod","cpu":2,"memory_gb":4,"build":{"context":".","dockerfile":"Dockerfile"}
    }))
    .unwrap();
    for successful_lookup in [false, true] {
        let cancel = Cancellation::default();
        let result = inspect_with(
            &fixture.cloud,
            &fixture.request.repository,
            "cpu",
            &profile,
            &cancel,
            |_, _| {
                cancel.cancel();
                if successful_lookup {
                    Ok(Selection::Automatic {
                        login: None,
                        repository: None,
                    })
                } else {
                    Err(horizon_cloud::CloudError::Cancelled.into())
                }
            },
        );
        assert!(matches!(
            result,
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
    }
}
