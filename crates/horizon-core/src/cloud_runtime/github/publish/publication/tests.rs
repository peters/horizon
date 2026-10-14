use super::*;
use base64::Engine as _;

fn account(id: u64, login: &str) -> Account {
    Account {
        id,
        login: login.into(),
    }
}

#[test]
fn account_identity_includes_the_immutable_provider_id() {
    let original = account(10, "alice");
    assert!(original.check(&account(10, "Alice")).is_ok());
    assert!(
        original.check(&account(11, "alice")).is_err(),
        "a reused name is not the account"
    );
    assert!(
        original.check(&account(10, "bob")).is_err(),
        "a rename cannot redirect the namespace"
    );
    assert!(account(0, "alice").check(&account(0, "alice")).is_err());
}

#[test]
fn an_account_mismatch_is_refused_before_preparing_a_login() {
    let expected = account(10, "alice");
    let result = prepare_with(Some(&expected), || {
        Ok((account(11, "bob"), Secret::new("synthetic-other-token".into())))
    });
    assert!(result.is_err());
}

#[test]
fn simultaneous_operations_keep_their_own_logins_and_leave_the_host_config_intact() {
    let root = tempfile::tempdir().unwrap();
    let original = br#"{"auths":{"registry.example":{"auth":"c3ludGhldGlj"}}}"#;
    super::super::write_private(root.path(), "config.json", original).unwrap();
    let first = prepare_with(Some(&account(10, "alice")), || {
        Ok((account(10, "alice"), Secret::new("synthetic-first-token".into())))
    })
    .unwrap();
    let second = prepare_with(Some(&account(11, "bob")), || {
        Ok((account(11, "bob"), Secret::new("synthetic-second-token".into())))
    })
    .unwrap();
    let read = |prepared: &Prepared| {
        let bytes = std::fs::read(prepared.docker_config().join("config.json")).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(json["auths"]["ghcr.io"]["auth"].as_str().unwrap())
            .unwrap()
    };
    assert_eq!(read(&first), b"alice:synthetic-first-token");
    assert_eq!(read(&second), b"bob:synthetic-second-token");
    assert_ne!(first.docker_config(), second.docker_config());
    assert_eq!(std::fs::read(root.path().join("config.json")).unwrap(), original);
    let first_path = first.docker_config().to_path_buf();
    drop(first);
    assert!(!first_path.exists());
    assert!(second.docker_config().is_dir());
}
