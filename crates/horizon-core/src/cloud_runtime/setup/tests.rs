use super::*;

fn prepared(root: &Path) -> Draft {
    let mut draft = Draft::load(root).unwrap();
    *draft.runpod_key = "synthetic-compute-key".into();
    // These tests exercise saving, not the platform ssh-keygen prerequisite.
    let identity = root.join("existing-identity");
    let file = tempfile::NamedTempFile::new_in(root).unwrap();
    std::fs::write(file.path(), "synthetic-private-identity").unwrap();
    file.persist(&identity).unwrap();
    draft.settings.ssh_identity_file = identity;
    draft
}

#[test]
fn first_use_keeps_secrets_out_of_settings_and_preserves_saved_bindings() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    draft.settings.default_agents = vec![Agent::Codex];
    draft.openai_auth = Authentication::ApiKey;
    *draft.openai_key = "synthetic-agent-key".into();
    let saved = draft.save().unwrap();
    let raw = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
    assert!(!raw.contains("synthetic"));
    assert_eq!(
        std::fs::read_to_string(&saved.runpod_key_file).unwrap(),
        "synthetic-compute-key"
    );
    let reopened = Draft::load(root.path()).unwrap();
    assert!(reopened.runpod_key.is_empty() && reopened.openai_key.is_empty());
    assert!(reopened.validate().is_ok());
    let resaved = reopened.save().unwrap();
    assert_eq!(resaved.openai_api_key_file, saved.openai_api_key_file);
    assert_eq!(resaved.runpod_key_file, saved.runpod_key_file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [
            saved.runpod_key_file,
            saved.openai_api_key_file.unwrap(),
            root.path().join("settings.json"),
        ] {
            assert_eq!(std::fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
        }
    }
}

#[test]
fn credentials_are_required_only_for_selected_api_agents() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    draft.openai_auth = Authentication::ApiKey;
    draft.anthropic_auth = Authentication::ApiKey;
    draft.settings.default_agents = vec![Agent::Codex];
    assert!(draft.validate().is_err());
    *draft.openai_key = "synthetic-agent-key".into();
    assert!(draft.validate().is_ok());
    draft.settings.default_agents.push(Agent::Claude);
    assert!(draft.validate().is_err());
    draft.anthropic_auth = Authentication::Subscription;
    assert!(draft.validate().is_ok());
    draft.settings.default_agents.clear();
    assert!(draft.validate().is_err());
}

#[test]
fn failed_or_stale_save_preserves_previous_settings_and_new_secrets_are_removed() {
    let root = tempfile::tempdir().unwrap();
    let saved = prepared(root.path()).save().unwrap();
    let mut first = Draft::load(root.path()).unwrap();
    let mut stale = Draft::load(root.path()).unwrap();
    *first.runpod_key = "replacement-compute-key".into();
    first.save().unwrap();
    let current = std::fs::read(root.path().join("settings.json")).unwrap();
    *stale.runpod_key = "stale-compute-key".into();
    assert!(stale.save().is_err());
    assert_eq!(std::fs::read(root.path().join("settings.json")).unwrap(), current);
    let mut invalid = Draft::load(root.path()).unwrap();
    *invalid.runpod_key = "never-committed-key".into();
    invalid.settings.ssh_identity_file = root.path().join("missing-custom-key");
    let before = std::fs::read_dir(root.path().join("credentials")).unwrap().count();
    assert!(invalid.save().is_err());
    assert_eq!(
        std::fs::read_dir(root.path().join("credentials")).unwrap().count(),
        before
    );
    assert_eq!(std::fs::read(root.path().join("settings.json")).unwrap(), current);
    assert!(
        saved.runpod_key_file.exists(),
        "An earlier binding must never be removed"
    );
}

#[test]
fn subscription_choice_removes_the_binding_without_deleting_an_external_key() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    draft.openai_auth = Authentication::ApiKey;
    *draft.openai_key = "synthetic-agent-key".into();
    let old = draft.save().unwrap().openai_api_key_file.unwrap();
    let mut draft = Draft::load(root.path()).unwrap();
    draft.openai_auth = Authentication::Subscription;
    assert!(draft.save().unwrap().openai_api_key_file.is_none());
    assert!(old.exists());
}

#[test]
fn malformed_settings_and_multiline_keys_are_not_silently_accepted() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("settings.json"), "invalid").unwrap();
    assert!(Draft::load(root.path()).is_err());
    for value in [" \n", "key\ninjected", "key with spaces"] {
        assert!(validate_input(value, None).is_err());
    }
}

#[test]
fn edits_during_secret_preparation_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let draft = prepared(root.path());
    let mut write = storage::Transaction::new(root.path()).unwrap();
    write.verify_current(None).unwrap();
    let secret = write.secret("compute", "synthetic-staged-key").unwrap();
    std::fs::write(root.path().join("settings.json"), "externally edited").unwrap();
    assert!(write.commit(&draft.settings, None).is_err());
    drop(write);
    assert!(!secret.exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("settings.json")).unwrap(),
        "externally edited"
    );
}

#[test]
#[cfg(unix)]
fn first_use_generates_a_dedicated_usable_private_identity() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = Draft::load(root.path()).unwrap();
    *draft.runpod_key = "synthetic-compute-key".into();
    let saved = draft.save().unwrap();
    assert!(saved.ssh_identity_file.starts_with(root.path()));
    let output = std::process::Command::new("ssh-keygen")
        .args(["-y", "-f"])
        .arg(&saved.ssh_identity_file)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().starts_with("ssh-ed25519 "));
    settings::validate_ssh_identity(&saved.ssh_identity_file).unwrap();
}
