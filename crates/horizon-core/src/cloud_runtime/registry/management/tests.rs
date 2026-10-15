use super::*;
use crate::cloud_runtime::registry::tests::fixture;

fn image() -> String {
    format!("registry.example/team/worker@sha256:{}", "a".repeat(64))
}

#[test]
#[cfg(unix)]
fn host_pull_status_needs_neither_a_compute_key_nor_live_secret_files() {
    let (root, settings) = fixture();
    std::fs::remove_file(&settings.runpod_key_file).unwrap();
    let binding = &settings.registries.as_ref().unwrap().bindings[0];
    std::fs::remove_file(&binding.pull.secret_file).unwrap();
    std::fs::remove_file(&binding.publish.as_ref().unwrap().secret_file).unwrap();
    let action = Action::PullStatus {
        repository: binding.repository.clone(),
        generation: binding.generation.clone(),
    };
    let status = manage(&settings, &action, &Cancellation::default()).unwrap();
    assert_eq!(status.scope, Scope::Host);
    assert!(matches!(status.state, State::Prepared));
    assert!(status.validation.is_none());
    assert!(!root.path().join("registry/generation1.json").exists());
    let provider_action = Action::Status {
        repository: binding.repository.clone(),
        generation: binding.generation.clone(),
    };
    assert!(manage(&settings, &provider_action, &Cancellation::default()).is_err());
}

#[test]
#[cfg(unix)]
fn successful_pull_verification_creates_no_provider_access_without_a_compute_key() {
    let (root, settings) = fixture();
    std::fs::remove_file(&settings.runpod_key_file).unwrap();
    let action = Action::VerifyPull { image: image() };
    let status = manage_with(&settings, &action, &Cancellation::default(), |prepared, image, _| {
        assert!(prepared.direct_host);
        prepared.journal.record_validation(Validation {
            image: image.into(),
            scope: "synthetic read-only grant".into(),
            expires_at: None,
            checked_at: 1,
        })
    })
    .unwrap();
    assert_eq!(status.scope, Scope::Host);
    assert!(matches!(status.state, State::Prepared));
    assert_eq!(status.validation.unwrap().image, image());
    assert!(root.path().join("registry/hosts/generation1.json").is_file());
    assert!(!root.path().join("registry/generation1.json").exists());
    let action = Action::PullStatus {
        repository: status.repository,
        generation: status.generation,
    };
    let status = manage(&settings, &action, &Cancellation::default()).unwrap();
    assert_eq!(status.validation.unwrap().scope, "synthetic read-only grant");
}

#[test]
#[cfg(unix)]
fn public_pull_verification_refuses_a_mutable_tag_before_docker_or_provider_use() {
    let (root, settings) = fixture();
    std::fs::remove_file(&settings.runpod_key_file).unwrap();
    let action = Action::VerifyPull {
        image: "registry.example/team/worker:latest".into(),
    };
    let error = manage(&settings, &action, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("immutable image"), "{error}");
    assert!(root.path().join("registry/hosts/generation1.json").is_file());
    assert!(!root.path().join("registry/generation1.json").exists());
}

#[test]
fn cancelled_pull_management_does_not_create_a_journal() {
    let (root, settings) = fixture();
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(manage(&settings, &Action::VerifyPull { image: image() }, &cancel).is_err());
    assert!(!root.path().join("registry").exists());
}
