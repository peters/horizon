//! The GitHub App installation token that an agent's cloud gets instead of a sign-in.
use super::*;

/// This machine's cloud settings, with an agent policy of one repository and, when `key` is
/// given, the GitHub App whose private key file it names.
fn machine_settings(root: &Path, key: Option<&Path>) -> Settings {
    let mut file = json!({
        "runpod_key_file": root.join("key"),
        "ssh_identity_file": root.join("identity"),
        "docker_config": root.join("docker"),
        "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
        "agent_lifecycle": policy(),
    });
    if let Some(key) = key {
        file["github"] = json!({
            "app_id": 42,
            "slug": "horizon-example",
            "client_id": "Iv23synthetic",
            "client_secret_file": root.join("client-secret"),
            "private_key_file": key,
        });
    }
    std::fs::write(root.join("settings.json"), file.to_string()).unwrap();
    Settings::for_cloud(&root.join("settings.json"), &Placement::default()).unwrap()
}

/// A quiet runner and the cancellation that owns it: the mint emits nothing here.
fn runner(cancel: &Cancellation) -> cloud_runtime::command::Runner<'_> {
    cloud_runtime::command::Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
}

/// A mint that answers as an app whose installation does not reach `acme/api`.
fn uncovered(
    _settings: &Settings,
    _runner: &cloud_runtime::command::Runner<'_>,
) -> std::result::Result<Option<github::Installation>, github::InstallationRefusal> {
    Err(github::InstallationRefusal::NotCovered {
        repositories: "acme/web, acme/api".into(),
        url: "https://github.com/apps/horizon-example/installations/new".into(),
    })
}

#[test]
fn an_agent_cloud_whose_app_does_not_cover_a_repository_is_refused_by_name() {
    let temp = tempfile::tempdir().unwrap();
    let settings = machine_settings(temp.path(), Some(&temp.path().join("app.pem")));
    let cancel = Cancellation::default();
    let refused = installation_access(&settings, &runner(&cancel), uncovered).unwrap_err();
    assert!(refused.starts_with("cloud_create_refused"), "{refused}");
    assert!(
        refused.contains("acme/api"),
        "the refusal names the repository that no token reaches: {refused}"
    );
}

#[test]
fn an_agent_cloud_without_the_apps_key_keeps_the_person_sign_in() {
    let temp = tempfile::tempdir().unwrap();
    let settings = machine_settings(temp.path(), None);
    let cancel = Cancellation::default();
    assert_eq!(
        installation_access(&settings, &runner(&cancel), github::for_agent_cloud),
        Ok(()),
        "without a key nothing is minted, so the worker still asks a person to sign in"
    );
}
