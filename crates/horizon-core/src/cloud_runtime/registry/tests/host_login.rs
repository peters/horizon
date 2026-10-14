use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
#[cfg(unix)]
fn a_host_login_requires_verification_and_keeps_the_verified_pull_material() {
    let (root, mut settings) = fixture();
    let mut prepared = Prepared::for_image(&settings, &image(), None, false).unwrap().unwrap();
    assert!(
        prepared.host_login().is_err(),
        "unverified material cannot reach a host"
    );
    assert_eq!(
        std::fs::metadata(prepared.docker_config(false))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let script = root.path().join("docker-fixture");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' '{{\"digest\":\"sha256:{}\"}}'\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepared
        .verify_image_with(&image(), &Cancellation::default(), Command::new(&script))
        .unwrap();
    let path = binding(&mut settings).pull.secret_file.clone();
    std::fs::write(&path, "another-account-pull").unwrap();
    let login = prepared.host_login().unwrap();
    assert_eq!(login.server, "registry.example");
    assert_eq!(login.username, "reader");
    let plan = horizon_cloud::host::Plan {
        image: image(),
        environment: std::collections::BTreeMap::new(),
        registry: Some(login),
        workspace_device: "/dev/disk/by-id/scsi-synthetic".into(),
        shm_gb: 1,
    };
    let yaml = plan.cloud_config().unwrap();
    let document: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let entry = document["write_files"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|entry| entry["path"].as_str() == Some("/root/.docker/config.json"))
        .unwrap();
    let json = base64::engine::general_purpose::STANDARD
        .decode(entry["content"].as_str().unwrap())
        .unwrap();
    let config: serde_json::Value = serde_json::from_slice(&json).unwrap();
    let auth = base64::engine::general_purpose::STANDARD
        .decode(config["auths"]["registry.example"]["auth"].as_str().unwrap())
        .unwrap();
    assert_eq!(auth, b"reader:synthetic-pull");
    assert!(!String::from_utf8(json).unwrap().contains("synthetic-push"));
    assert!(root.path().join("registry").exists());
}

#[test]
fn an_unconfirmed_or_expired_binding_cannot_supply_a_host_login() {
    let (_root, mut settings) = fixture();
    binding(&mut settings).read_only_confirmed = false;
    assert!(binding(&mut settings).host_login().is_err());
    binding(&mut settings).read_only_confirmed = true;
    binding(&mut settings).pull.expires_at = Some("2000-01-01T00:00:00Z".into());
    assert!(binding(&mut settings).host_login().is_err());
}
