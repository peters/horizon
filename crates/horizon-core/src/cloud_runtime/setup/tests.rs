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

/// Gives Codex and Claude synthetic API keys, so a save reaches the concern the test
/// actually exercises instead of stopping at the agent credentials.
fn with_agent_credentials(draft: &mut Draft) {
    draft.openai_auth = Authentication::ApiKey;
    *draft.openai_key = "synthetic-codex-key".into();
    draft.anthropic_auth = Authentication::ApiKey;
    *draft.anthropic_key = "synthetic-claude-key".into();
}

#[test]
fn a_saved_codex_subscription_login_becomes_the_chatgpt_sign_in() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    draft.settings.default_agents = vec![Agent::Codex];
    draft.openai_auth = Authentication::Subscription;
    draft.save().unwrap();
    let reopened = Draft::load(root.path()).unwrap();
    assert_eq!(
        reopened.openai_auth,
        Authentication::ChatGpt,
        "the old Codex login migrates to the ChatGPT sign-in"
    );
}

#[test]
fn a_codex_setting_without_a_key_derives_the_chatgpt_sign_in() {
    let root = tempfile::tempdir().unwrap();
    let draft = prepared(root.path());
    assert_eq!(
        draft.openai_auth,
        Authentication::ChatGpt,
        "without an API key the ChatGPT sign-in is the Codex login"
    );
}

#[test]
fn an_unavailable_account_status_is_not_reported_as_signed_out() {
    let root = tempfile::tempdir().unwrap();
    assert!(super::super::chatgpt::confirm_usage(root.path(), "missing-client").is_err());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.path().join("chatgpt/session.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let mut draft = prepared(root.path());
    draft.settings.default_agents = vec![Agent::Codex];
    assert!(draft.chatgpt.is_none());
    assert!(draft.chatgpt_status_error.as_deref().unwrap().contains("in progress"));
    assert!(draft.validate().unwrap_err().to_string().contains("status unavailable"));
    drop(lock);
    let reopened = Draft::load(root.path()).unwrap();
    assert!(reopened.chatgpt.is_none());
    assert!(reopened.chatgpt_status_error.is_none());
}

#[test]
fn unsupported_plan_authentication_preserves_saved_agent_credentials() {
    let root = tempfile::tempdir().unwrap();
    let mut base = prepared(root.path());
    with_agent_credentials(&mut base);
    let saved = base.save().unwrap();
    let settings = std::fs::read(root.path().join("settings.json")).unwrap();
    let key_path = saved.anthropic_api_key_file.unwrap();
    let key = std::fs::read(&key_path).unwrap();
    for agents in [vec![Agent::Claude], vec![Agent::Codex]] {
        let mut draft = Draft::load(root.path()).unwrap();
        draft.settings.default_agents = agents;
        draft.anthropic_auth = Authentication::ChatGpt;
        draft.chatgpt = Some(super::super::chatgpt::Connection {
            client_id: "oaiapp_synthetic".into(),
            email: None,
            subject: "synthetic-user".into(),
            scopes: vec!["chatgpt.tokens.use.direct".into()],
            plan_usage: true,
            usage_confirmed: true,
            signed_in: true,
            saved_at_unix: 1,
        });
        assert!(
            draft
                .validate()
                .unwrap_err()
                .to_string()
                .contains("supported only for Codex")
        );
        assert!(draft.save().is_err());
        assert_eq!(std::fs::read(root.path().join("settings.json")).unwrap(), settings);
        assert_eq!(std::fs::read(&key_path).unwrap(), key);
    }
}

#[test]
fn plan_save_requires_a_renewable_sign_in_and_plan_grant_without_clearing_keys() {
    let root = tempfile::tempdir().unwrap();
    let mut base = prepared(root.path());
    with_agent_credentials(&mut base);
    let saved = base.save().unwrap();
    let settings = std::fs::read(root.path().join("settings.json")).unwrap();
    let key_path = saved.openai_api_key_file.unwrap();
    let key = std::fs::read(&key_path).unwrap();
    for (signed_in, plan_usage) in [(true, false), (false, true)] {
        let mut draft = Draft::load(root.path()).unwrap();
        draft.settings.default_agents = vec![Agent::Codex];
        draft.openai_auth = Authentication::ChatGpt;
        draft.chatgpt = Some(super::super::chatgpt::Connection {
            client_id: "oaiapp_synthetic".into(),
            email: None,
            subject: "synthetic-user".into(),
            scopes: if plan_usage {
                vec!["chatgpt.tokens.use.direct".into()]
            } else {
                vec![]
            },
            plan_usage,
            usage_confirmed: true,
            signed_in,
            saved_at_unix: 1,
        });
        assert!(draft.validate().unwrap_err().to_string().contains("grant plan access"));
        let mut eligible = draft.clone();
        let connection = eligible.chatgpt.as_mut().unwrap();
        connection.signed_in = true;
        connection.plan_usage = true;
        assert!(eligible.validate().is_ok());
        assert!(draft.save().is_err());
        assert_eq!(std::fs::read(root.path().join("settings.json")).unwrap(), settings);
        assert_eq!(std::fs::read(&key_path).unwrap(), key);
    }
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
    let mut base = prepared(root.path());
    with_agent_credentials(&mut base);
    let saved = base.save().unwrap();
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
    with_agent_credentials(&mut draft);
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

#[test]
fn hetzner_is_optional_and_its_token_stays_a_private_secret() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    with_agent_credentials(&mut draft);
    assert!(!draft.hetzner.enabled, "new settings start without Hetzner");
    assert_eq!(draft.hetzner.locations, "hel1, nbg1, fsn1");
    let saved = draft.save().unwrap();
    assert!(saved.hetzner.is_none());

    // Turning Hetzner on needs a token.
    let mut draft = Draft::load(root.path()).unwrap();
    draft.hetzner.enabled = true;
    assert_eq!(
        draft.validate().unwrap_err().to_string(),
        "Enter your Hetzner Cloud API token"
    );
    *draft.hetzner.token = "two words".into();
    assert!(draft.validate().is_err(), "a token is one line without spaces");
    *draft.hetzner.token = "synthetic-hetzner-token".into();
    draft.hetzner.server_types = "cx43,  cpx42".into();
    draft.hetzner.locations = "HEL1".into();
    assert!(draft.validate().is_err(), "Hetzner names are lowercase");
    draft.hetzner.locations = "hel1, nbg1".into();
    let saved = draft.save().unwrap();
    let hetzner = saved.hetzner.unwrap();
    assert_eq!(
        (hetzner.server_types, hetzner.locations),
        (
            vec!["cx43".to_owned(), "cpx42".to_owned()],
            vec!["hel1".to_owned(), "nbg1".to_owned()]
        )
    );
    assert_eq!(
        std::fs::read_to_string(&hetzner.token_file).unwrap(),
        "synthetic-hetzner-token"
    );
    assert!(
        !std::fs::read_to_string(root.path().join("settings.json"))
            .unwrap()
            .contains("synthetic-hetzner")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&hetzner.token_file).unwrap().permissions().mode() & 0o077,
            0
        );
    }

    // Reopened, the saved token is kept without being shown, and turning Hetzner off
    // removes the binding.
    let mut reopened = Draft::load(root.path()).unwrap();
    assert!(reopened.hetzner.enabled && reopened.hetzner.token.is_empty());
    assert_eq!(reopened.hetzner.server_types, "cx43, cpx42");
    assert_eq!(
        reopened.clone().save().unwrap().hetzner.unwrap().token_file,
        hetzner.token_file
    );
    reopened.hetzner.enabled = false;
    assert!(reopened.save().unwrap().hetzner.is_none());
}

#[test]
fn editing_hetzner_keeps_its_registry_pull_binding() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    with_agent_credentials(&mut draft);
    draft.hetzner.enabled = true;
    *draft.hetzner.token = "synthetic-hetzner-token".into();
    let mut saved = draft.save().unwrap();
    let password = root.path().join("registry-password");
    std::fs::write(&password, "synthetic-registry-password").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let pull = crate::cloud_runtime::settings::RegistryPull {
        server: "registry.example.com".into(),
        username: "puller".into(),
        password_file: password,
    };
    saved.hetzner.as_mut().unwrap().registry_pull = Some(pull.clone());
    std::fs::write(root.path().join("settings.json"), serde_json::to_vec(&saved).unwrap()).unwrap();

    // Editing the lists and leaving the token blank keeps the pull binding.
    let mut edited = Draft::load(root.path()).unwrap();
    edited.hetzner.server_types = "cpx42".into();
    let resaved = edited.save().unwrap();
    let hetzner = resaved.hetzner.unwrap();
    assert_eq!(hetzner.server_types, ["cpx42"]);
    assert_eq!(hetzner.registry_pull, Some(pull.clone()));
    // A new token keeps it too.
    let mut rekeyed = Draft::load(root.path()).unwrap();
    *rekeyed.hetzner.token = "another-synthetic-token".into();
    assert_eq!(rekeyed.save().unwrap().hetzner.unwrap().registry_pull, Some(pull));
}

#[test]
fn a_machine_can_be_set_up_for_hetzner_alone() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    with_agent_credentials(&mut draft);
    draft.runpod_key.clear();
    // Neither provider: one is required.
    let refused = draft.validate().unwrap_err().to_string();
    assert!(
        refused.contains("RunPod API key") && refused.contains("Hetzner"),
        "{refused}"
    );
    draft.hetzner.enabled = true;
    *draft.hetzner.token = "synthetic-hetzner-token".into();
    let saved = draft.save().unwrap();
    assert!(!saved.runpod_configured() && saved.hetzner.is_some());
    // RunPod requests say what is missing instead of failing on a missing file.
    assert_eq!(
        saved.credential().unwrap_err().to_string(),
        crate::cloud_runtime::settings::RUNPOD_KEY_MISSING
    );
    // Reopened, the settings stay valid, and turning Hetzner off needs a RunPod key again.
    let mut reopened = Draft::load(root.path()).unwrap();
    reopened.validate().unwrap();
    reopened.hetzner.enabled = false;
    assert!(reopened.validate().is_err());
    *reopened.runpod_key = "synthetic-compute-key".into();
    assert!(reopened.save().unwrap().runpod_configured());
}

#[test]
fn saved_credential_hints_require_available_files_when_reopened() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = prepared(root.path());
    draft.openai_auth = Authentication::ApiKey;
    *draft.openai_key = "synthetic-agent-key".into();
    let saved = draft.save().unwrap();
    let agent = saved.openai_api_key_file.as_ref().unwrap();
    let loaded = Draft::load(root.path()).unwrap();
    assert!(loaded.saved_credentials.contains(&saved.runpod_key_file));
    assert!(loaded.saved_credentials.contains(agent));
    std::fs::remove_file(agent).unwrap();
    std::fs::write(&saved.runpod_key_file, "").unwrap();
    let reopened = Draft::load(root.path()).unwrap();
    assert!(!reopened.saved_credentials.contains(agent));
    assert!(!reopened.saved_credentials.contains(&saved.runpod_key_file));
    assert!(reopened.settings.openai_api_key_file.is_some());
    assert!(reopened.validate().is_err());
}

#[test]
#[cfg(unix)]
fn a_first_cloud_needs_only_one_provider_key() {
    // The SSH identity is made by the platform's ssh-keygen; without it there is nothing to check.
    if std::process::Command::new("ssh-keygen").arg("-?").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let saved = save_provider_key(root.path(), Provider::RunPod, " synthetic-compute-key\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(&saved.runpod_key_file).unwrap(),
        "synthetic-compute-key"
    );
    assert!(saved.ssh_identity_file.is_file());
    let raw = std::fs::read_to_string(root.path().join("settings.json")).unwrap();
    assert!(!raw.contains("synthetic"));
}

#[test]
#[cfg(unix)]
fn a_hetzner_token_saved_alone_turns_hetzner_on_without_a_runpod_key() {
    if std::process::Command::new("ssh-keygen").arg("-?").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let saved = save_provider_key(root.path(), Provider::Hetzner, "synthetic-hetzner-token").unwrap();
    let hetzner = saved.hetzner.as_ref().expect("Hetzner is bound");
    assert_eq!(
        std::fs::read_to_string(&hetzner.token_file).unwrap(),
        "synthetic-hetzner-token"
    );
    assert!(!saved.runpod_configured());
    assert!(
        !std::fs::read_to_string(root.path().join("settings.json"))
            .unwrap()
            .contains("synthetic")
    );
}

#[test]
fn an_open_form_keeps_saving_after_connect_github_saved_the_app() {
    let root = tempfile::tempdir().unwrap();
    let mut base = prepared(root.path());
    with_agent_credentials(&mut base);
    base.save().unwrap();
    let mut form = Draft::load(root.path()).unwrap();
    let app = crate::cloud_runtime::github::Settings {
        app_id: 42,
        slug: "horizon-example".into(),
        client_id: "Iv23synthetic".into(),
        client_secret_file: root.path().join("credentials/github-app"),
        mode: crate::cloud_runtime::github::Mode::Ask,
    };
    let committed = save_github(
        root.path(),
        None,
        Some(app.clone()),
        &crate::cloud_runtime::Cancellation::default(),
    )
    .unwrap();
    form.adopt_github(&committed, Some(app.clone()));
    form.settings.github.as_mut().unwrap().mode = crate::cloud_runtime::github::Mode::Automatic;
    let saved = form
        .save()
        .expect("the form's save is not taken for a change made elsewhere");
    assert_eq!(
        saved.github.map(|github| github.mode),
        Some(crate::cloud_runtime::github::Mode::Automatic)
    );
    // A form opened before another change still sees that change as made elsewhere.
    let mut stale = Draft::load(root.path()).unwrap();
    let other = save_github(
        root.path(),
        Some(42),
        None,
        &crate::cloud_runtime::Cancellation::default(),
    )
    .unwrap();
    std::fs::write(root.path().join("settings.json"), b"{}").unwrap();
    stale.adopt_github(&other, None);
    assert!(stale.save().is_err());
}

#[test]
fn connect_github_saves_before_any_provider_is_set_up() {
    let root = tempfile::tempdir().unwrap();
    let app = crate::cloud_runtime::github::Settings {
        app_id: 42,
        slug: "horizon-example".into(),
        client_id: "Iv23synthetic".into(),
        client_secret_file: root.path().join("credentials/github-app-42"),
        mode: crate::cloud_runtime::github::Mode::Ask,
    };
    let cancelled = crate::cloud_runtime::Cancellation::default();
    cancelled.cancel();
    assert!(save_github(root.path(), None, Some(app.clone()), &cancelled).is_err());
    assert!(
        !root.path().join("settings.json").exists(),
        "a cancelled flow saves nothing"
    );
    save_github(
        root.path(),
        None,
        Some(app.clone()),
        &crate::cloud_runtime::Cancellation::default(),
    )
    .expect("no RunPod key is needed");
    let draft = Draft::load(root.path()).unwrap();
    assert_eq!(draft.settings.github, Some(app));
    assert!(!draft.settings.runpod_configured());
}

#[test]
fn a_github_save_applies_only_while_the_settings_name_the_shown_app() {
    let root = tempfile::tempdir().unwrap();
    let app = |app_id| crate::cloud_runtime::github::Settings {
        app_id,
        slug: format!("horizon-example-{app_id}"),
        client_id: "Iv23synthetic".into(),
        client_secret_file: root.path().join(format!("credentials/github-app-{app_id}")),
        mode: crate::cloud_runtime::github::Mode::Ask,
    };
    let cancel = crate::cloud_runtime::Cancellation::default();
    // Another window connected app 43 while this one showed app 42, or no app.
    save_github(root.path(), None, Some(app(43)), &cancel).unwrap();
    let automatic = crate::cloud_runtime::github::Settings {
        mode: crate::cloud_runtime::github::Mode::Automatic,
        ..app(42)
    };
    assert!(save_github(root.path(), Some(42), Some(automatic), &cancel).is_err());
    assert!(save_github(root.path(), Some(42), None, &cancel).is_err());
    assert!(
        save_github(root.path(), None, Some(app(44)), &cancel).is_err(),
        "a stale Connect never replaces a connected app"
    );
    assert_eq!(Draft::load(root.path()).unwrap().settings.github, Some(app(43)));
    // The app the settings name changes as asked.
    save_github(root.path(), Some(43), None, &cancel).expect("the shown app is still the saved one");
    assert_eq!(Draft::load(root.path()).unwrap().settings.github, None);
    assert!(
        save_github(root.path(), Some(43), None, &cancel).is_err(),
        "a second Disconnect finds nothing to change"
    );
}
