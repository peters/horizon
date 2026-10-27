//! The GitHub chain that an agent's cloud gets from this computer's own sign-in.
use super::*;

/// This machine's cloud settings, with an agent policy of one repository and, when `key` is
/// given, the GitHub App whose sign-in the chain belongs to.
fn machine_settings(root: &Path, app: bool) -> Settings {
    let mut file = json!({
        "runpod_key_file": root.join("key"),
        "ssh_identity_file": root.join("identity"),
        "docker_config": root.join("docker"),
        "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
        "agent_lifecycle": policy(),
    });
    if app {
        file["github"] = json!({
            "app_id": 42,
            "slug": "horizon-example",
            "client_id": "Iv23synthetic",
            "client_secret_file": root.join("client-secret"),
            "private_key_file": root.join("app.pem"),
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

/// A mint that answers as a sign-in that did not finish.
fn unsigned(
    _settings: &Settings,
    _runner: &cloud_runtime::command::Runner<'_>,
) -> std::result::Result<Option<github::PersonAccess>, github::PersonChainRefusal> {
    Err(github::PersonChainRefusal::SignIn(
        "You declined the sign-in on GitHub.".into(),
    ))
}

#[test]
fn an_agent_cloud_whose_sign_in_does_not_finish_is_refused_and_rents_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let settings = machine_settings(temp.path(), true);
    let cancel = Cancellation::default();
    let refused = github_access(&settings, &runner(&cancel), unsigned).unwrap_err();
    assert!(refused.starts_with("cloud_create_refused"), "{refused}");
    assert!(
        refused.contains("You declined the sign-in on GitHub."),
        "the refusal carries the reason the sign-in ended: {refused}"
    );
    assert!(
        refused.contains("agent_lifecycle.allow_github_sign_in_skip"),
        "the refusal names the setting that would let the cloud continue without GitHub access: {refused}"
    );
}

#[test]
fn a_cloud_that_may_continue_without_github_access_gets_no_chain_and_is_not_refused() {
    let temp = tempfile::tempdir().unwrap();
    let settings = machine_settings(temp.path(), false);
    let cancel = Cancellation::default();
    // No chain is taken, so the worker still asks a person to sign in, which the policy of
    // this test may skip.
    let none = |_: &Settings,
                _: &cloud_runtime::command::Runner<'_>|
     -> std::result::Result<Option<github::PersonAccess>, github::PersonChainRefusal> { Ok(None) };
    assert_eq!(github_access(&settings, &runner(&cancel), none), Ok(()));
}

#[test]
fn the_line_of_a_sign_in_names_the_code_and_the_page() {
    let device = Prompt::Device {
        user_code: "WDJB-MJHT".into(),
        verification_uri: "https://github.com/login/device".into(),
        expires_at: std::time::SystemTime::UNIX_EPOCH,
    };
    let line = sign_in_line(&device);
    assert!(line.contains("WDJB-MJHT"), "{line}");
    assert!(line.contains("https://github.com/login/device"), "{line}");
    assert!(line.contains("continues when the sign-in is approved"), "{line}");
    let web = sign_in_line(&Prompt::Web {
        url: "https://github.com/login/oauth/authorize?client_id=Iv23synthetic".into(),
    });
    assert!(web.contains("https://github.com/login/oauth/authorize"), "{web}");
}
