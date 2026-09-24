use super::*;
#[cfg(unix)]
use base64::Engine as _;

#[test]
fn github_pull_grants_reject_write_delete_and_unknown_scopes() {
    assert!(credentials::verify_github_scopes(Some("read:packages")).is_ok());
    for scopes in [
        None,
        Some(""),
        Some("read:packages, write:packages"),
        Some("read:packages, delete:packages"),
        Some("repo, read:packages"),
    ] {
        assert!(credentials::verify_github_scopes(scopes).is_err());
    }
}

fn private_file(root: &Path, name: &str, value: &str) -> PathBuf {
    let file = tempfile::NamedTempFile::new_in(root).unwrap();
    std::fs::write(file.path(), value).unwrap();
    let path = root.join(name);
    file.persist(&path).unwrap();
    path
}

fn fixture() -> (tempfile::TempDir, Settings) {
    let root = tempfile::tempdir().unwrap();
    let path = |name| root.path().join(name);
    let compute = private_file(root.path(), "compute", "synthetic-compute");
    let pull = private_file(root.path(), "pull", "synthetic-pull");
    let push = private_file(root.path(), "push", "synthetic-push");
    let settings = serde_json::from_value(serde_json::json!({
        "runpod_key_file":compute,"ssh_identity_file":path("identity"),"docker_config":path("legacy-docker"),
        "registry_pull_auth_id":null,"cpu_flavors":[],"gpu_types":[],
        "registries":{"root":path("registry"),"bindings":[{
            "repository":"registry.example/team/worker", "generation":"generation1", "read_only_confirmed":true,
            "pull":{"username":"reader","secret_file":pull,"expires_at":null},
            "publish":{"username":"writer","secret_file":push,"expires_at":null}
        }]}
    }))
    .unwrap();
    (root, settings)
}

fn binding(settings: &mut Settings) -> &mut Binding {
    &mut settings.registries.as_mut().unwrap().bindings[0]
}
fn image() -> String {
    format!("registry.example/team/worker@sha256:{}", "a".repeat(64))
}

#[test]
fn exact_scope_rejects_neighbors_and_preserves_unbound_public_registries() {
    let (_root, settings) = fixture();
    let config = settings.registries.as_ref().unwrap();
    assert!(config.select(&image()).unwrap().is_some());
    assert!(config.select("registry.example/team/other:latest").is_err());
    assert!(config.select("registry.example/team/worker-evil:latest").is_err());
    for reference in [
        "ubuntu:22.04",
        "namespace/worker:tag",
        "elsewhere.example/team/worker:tag",
    ] {
        assert!(config.select(reference).unwrap().is_none());
    }
}

#[test]
fn generations_must_be_persisted_and_metadata_contains_no_secret() {
    let (_root, settings) = fixture();
    let json = serde_json::to_string(&settings).unwrap();
    assert!(!json.contains("synthetic-pull"));
    let mut json = serde_json::to_value(&settings).unwrap();
    json["registries"]["bindings"][0]
        .as_object_mut()
        .unwrap()
        .remove("generation");
    assert!(serde_json::from_value::<Settings>(json).is_err());
}

#[test]
fn expired_or_shared_pull_grants_fail_before_provider_contact() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).pull.expires_at = Some("2000-01-01T00:00:00Z".into());
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
    binding(&mut settings).pull.expires_at = None;
    let push = binding(&mut settings).publish.as_ref().unwrap().secret_file.clone();
    binding(&mut settings).pull.secret_file = push;
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
}

#[test]
fn secrets_in_source_and_unconfirmed_pull_grants_are_refused() {
    let (root, mut settings) = fixture();
    assert!(Prepared::for_image(&settings, &image(), Some(root.path()), false).is_err());
    binding(&mut settings).read_only_confirmed = false;
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn pull_only_work_does_not_require_a_live_publishing_credential() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).publish.as_mut().unwrap().expires_at = Some("2000-01-01T00:00:00Z".into());
    assert!(Prepared::for_image(&settings, &image(), None, false).is_ok());
    assert!(Prepared::for_image(&settings, &image(), None, true).is_err());
    std::fs::remove_file(&binding(&mut settings).publish.as_ref().unwrap().secret_file).unwrap();
    assert!(Prepared::for_image(&settings, &image(), None, false).is_ok());
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn generation_fences_changed_login_and_survives_reload() {
    let (_root, mut settings) = fixture();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    prepared
        .journal
        .save(&State::Requested {
            name: "horizon-pull-generation1".into(),
        })
        .unwrap();
    assert!(matches!(
        Prepared::for_image(&settings, &image(), None, false),
        Err(Error::Busy)
    ));
    drop(prepared);
    binding(&mut settings).pull.username = "another-reader".into();
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
    binding(&mut settings).pull.username = "reader".into();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    assert!(matches!(prepared.journal.state(), State::Requested { .. }));
    prepared.journal.save(&State::Revoked).unwrap();
    drop(prepared);
    assert!(Prepared::for_image(&settings, &image(), None, false).is_err());
}

#[test]
fn rotation_retains_old_handles_and_does_not_change_old_material() {
    let (root, mut settings) = fixture();
    let original = binding(&mut settings).clone();
    let mut draft = draft::Draft::from_binding(&original);
    assert!(draft.is_saved());
    draft.pull_secret = zeroize::Zeroizing::new("replacement-pull".into());
    assert!(!draft.is_saved());
    let rotated = draft
        .save(|name, value| Ok(private_file(root.path(), name, value)))
        .unwrap();
    assert_ne!(rotated.generation, original.generation);
    assert_eq!(rotated.retired, [original.generation]);
    assert_eq!(
        std::fs::read_to_string(original.pull.secret_file).unwrap(),
        "synthetic-pull"
    );
    assert_eq!(
        std::fs::read_to_string(rotated.pull.secret_file).unwrap(),
        "replacement-pull"
    );
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Cloud journals require Unix directory durability, matching deployment support"
)]
fn provider_transfer_requires_completed_image_validation() {
    let (_root, settings) = fixture();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    let provider = RunPod::new(settings.credential().unwrap());
    assert!(prepared.ensure_provider(&provider, &Cancellation::default()).is_err());
}

#[cfg(unix)]
#[test]
fn immutable_pull_probe_uses_only_the_pull_config_and_handles_failure_and_cancellation() {
    use std::os::unix::fs::PermissionsExt;
    let (root, mut settings) = fixture();
    settings.docker_host = Some("unix:///synthetic/docker.sock".into());
    let mut prepared = Prepared::for_image(&settings, &image(), None, true).unwrap().unwrap();
    assert_ne!(prepared.docker_config(true), prepared.docker_config(false));
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(prepared.docker_config(false).join("config.json")).unwrap()).unwrap();
    let auth = base64::engine::general_purpose::STANDARD
        .decode(config["auths"]["registry.example"]["auth"].as_str().unwrap())
        .unwrap();
    assert_eq!(String::from_utf8(auth).unwrap(), "reader:synthetic-pull");
    let script = root.path().join("docker-fixture");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n[ -z \"${{DOCKER_AUTH_CONFIG+x}}\" ] || exit 2\n[ \"$1\" = --host ] && [ \"$2\" = unix:///synthetic/docker.sock ] || exit 1\nprintf '%s' '{{\"digest\":\"sha256:{}\"}}'\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepared
        .verify_image_with(&image(), &Cancellation::default(), {
            let mut command = Command::new(&script);
            command.env("DOCKER_AUTH_CONFIG", "ambient-credential");
            command
        })
        .unwrap();
    assert!(prepared.verified);
    assert_eq!(prepared.journal.validation().unwrap().image, image());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        prepared
            .verify_image_with(&image(), &cancel, Command::new(&script))
            .is_err()
    );
    assert!(!prepared.verified);
    std::fs::write(&script, "#!/bin/sh\n[ \"$1\" = --host ] && [ \"$2\" = unix:///synthetic/docker.sock ] || exit 1\nprintf synthetic-pull >&2\nexit 1\n").unwrap();
    let error = prepared
        .verify_image_with(&image(), &Cancellation::default(), Command::new(&script))
        .unwrap_err();
    assert!(!error.to_string().contains("synthetic-pull"));
    assert!(!prepared.verified);
}

#[test]
fn pull_only_deployment_still_excludes_the_publishing_secret_from_source() {
    let (root, mut settings) = fixture();
    let source = root.path().join("repository");
    std::fs::create_dir(&source).unwrap();
    let path = private_file(&source, "push-token", "synthetic-push");
    binding(&mut settings).publish.as_mut().unwrap().secret_file = path;
    assert!(matches!(
        Prepared::for_image(&settings, &image(), Some(&source), false),
        Err(Error::Invalid(
            "Registry secrets must be outside the source and build context"
        ))
    ));
}

#[test]
fn equivalent_issuer_hosts_cannot_bypass_scope_policy() {
    for reference in [
        "ghcr.io/team/worker",
        "ghcr.io:443/team/worker",
        "GHCR.IO/team/worker",
        "ghcr.io./team/worker",
    ] {
        assert!(credentials::is_github_registry(reference));
    }
    assert!(!credentials::is_github_registry("ghcr.io.example/team/worker"));
    let (_root, mut settings) = fixture();
    binding(&mut settings).repository = "ghcr.io/team/worker".into();
    assert!(
        settings
            .registries
            .as_ref()
            .unwrap()
            .select("ghcr.io:443/team/other:latest")
            .is_err()
    );
}

#[test]
fn docker_hub_material_uses_the_canonical_login_key() {
    let (_root, settings) = fixture();
    let auth = &settings.registries.as_ref().unwrap().bindings[0].pull;
    let material = Material::load(auth, "docker.io/team/worker", None).unwrap();
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(material.config.path().join("config.json")).unwrap()).unwrap();
    assert!(config["auths"]["https://index.docker.io/v1/"]["auth"].is_string());
    assert_eq!(config["auths"].as_object().unwrap().len(), 1);
    assert_eq!(
        credentials::docker_auth_key("registry.example:5000/team/worker"),
        "registry.example:5000"
    );
}

#[test]
fn duplicate_current_and_retired_generations_are_rejected_globally() {
    let (_root, mut settings) = fixture();
    let mut second = binding(&mut settings).clone();
    second.repository = "registry.example/team/other".into();
    let config = settings.registries.as_mut().unwrap();
    config.bindings.push(second);
    assert!(config.validate().is_err());
    config.bindings[1].generation = "generation2".into();
    assert!(config.validate().is_ok());
    config.bindings[1].retired.push("generation1".into());
    assert!(config.validate().is_err());
}

#[cfg(unix)]
#[test]
fn status_exposes_configured_expiry_before_verification_without_mislabeling_retired_grants() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).pull.expires_at = Some("2030-01-01T00:00:00Z".into());
    binding(&mut settings).retired.push("previous".into());
    let status = |generation: &str| {
        manage(
            &settings,
            &Action::Status {
                repository: "registry.example/team/worker".into(),
                generation: generation.into(),
            },
            &Cancellation::default(),
        )
        .unwrap()
    };
    let current = status("generation1");
    assert!(current.validation.is_none());
    assert_eq!(current.configured_pull_expiry.as_deref(), Some("2030-01-01T00:00:00Z"));
    assert!(status("previous").configured_pull_expiry.is_none());
}
