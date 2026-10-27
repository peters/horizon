//! The chain of this computer's own GitHub sign-in, as an agent's cloud takes it. This
//! module is not named after that module, because a test module of that name would hide it.
use super::*;
use std::{
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

/// Where this computer stores its sign-in for the app of the fixture, `42`.
fn sign_in_path(directory: &Path) -> PathBuf {
    directory.join("credentials/github-host-42.json")
}

/// The chain that this computer stores for `login`, as its own sign-in stores one, and an
/// access token that lasts beyond the renewal margin so that reading it calls GitHub once.
fn store_chain(directory: &Path, login: &str) -> Chain {
    let now = SystemTime::now();
    let chain = Chain {
        access_token: Secret::new("ghu_synthetic_person".into()),
        access_expires_at: now + Duration::from_hours(2),
        refresh_token: Secret::new("ghr_synthetic_person".into()),
        refresh_expires_at: now + Duration::from_hours(4000),
    };
    stored::save(&sign_in_path(directory), login, &chain, false).unwrap();
    chain
}

fn seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// This machine's cloud settings: the GitHub App, and an agent policy that lists
/// `repositories` and may or may not allow a create to continue without GitHub access.
fn machine(
    directory: &std::path::Path,
    repositories: &[&std::path::Path],
    skippable: bool,
) -> crate::cloud_runtime::settings::Settings {
    let value = serde_json::json!({
        "runpod_key_file": directory.join("runpod-key"),
        "ssh_identity_file": directory.join("identity"),
        "docker_config": directory.join("docker"),
        "registry_pull_auth_id": null,
        "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
        "github": {
            "app_id": 42,
            "slug": "horizon-example",
            "client_id": "Iv23synthetic",
            "client_secret_file": directory.join("client-secret"),
                    },
        "agent_lifecycle": {
            "providers": ["runpod"],
            "max_hourly": 0.5,
            "repositories": repositories,
            "allow_github_sign_in_skip": skippable,
        },
    });
    let path = directory.join("settings.json");
    std::fs::write(&path, value.to_string()).unwrap();
    crate::cloud_runtime::settings::Settings::load(&path).unwrap()
}

#[test]
fn a_stored_chain_is_installed_as_the_person_with_a_payload_of_version_one() {
    let directory = tempfile::tempdir().unwrap();
    let web = checkout(directory.path(), "web", "git@github.com:Acme/Web.git");
    let settings = machine(directory.path(), &[&web], false);
    let stored_chain = store_chain(directory.path(), "octo-cat");
    let (client, task) = github(vec![serde_json::json!({
        "id": 7, "login": "octo-cat", "name": "Octo Cat"
    })]);
    let (cancel, _) = runner_parts();
    let access = person_chain::for_agent_cloud_at(&client, &settings, &quiet(&cancel))
        .unwrap()
        .expect("this computer holds a chain");
    task.join().unwrap();
    assert_eq!(access.user.login, "octo-cat");
    assert_eq!(access.user.name, "Octo Cat");
    assert_eq!(
        access.user.email, "7+octo-cat@users.noreply.github.com",
        "the address that GitHub attributes to the account"
    );
    assert_eq!(access.repositories, ["acme/web"], "the repositories of the policy");
    assert!(
        access.client_secret.is_none(),
        "a device chain renews on the worker with the client ID alone"
    );
    let grants = [Grant {
        repository: "acme/web".into(),
        target: Target::Primary,
    }];
    let payload = worker::payload(
        &access.app,
        access.client_secret.as_ref(),
        &access.user,
        &grants,
        &access.chain,
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(value["version"], 1, "the worker renews a person chain of version 1");
    assert_eq!(value["client_id"], "Iv23synthetic");
    assert_eq!(value["login"], "octo-cat");
    assert_eq!(value["author_name"], "Octo Cat");
    assert_eq!(value["author_email"], "7+octo-cat@users.noreply.github.com");
    assert_eq!(value["chain"]["access_token"], "ghu_synthetic_person");
    assert_eq!(value["chain"]["refresh_token"], "ghr_synthetic_person");
    assert_eq!(
        value["chain"]["access_expires_at"],
        seconds(stored_chain.access_expires_at)
    );
    assert_eq!(
        value["chain"]["refresh_expires_at"],
        seconds(stored_chain.refresh_expires_at),
        "the worker holds the refresh token and the end GitHub states for it"
    );
    assert_eq!(value["grants"][0]["repository"], "acme/web");
    assert_eq!(value["grants"][0]["target"], "primary");
    assert_eq!(value["grants"][0]["access"], "push");
}

#[test]
fn a_policy_that_allows_a_skip_keeps_a_cloud_without_a_stored_chain_without_github_access() {
    let directory = tempfile::tempdir().unwrap();
    let web = checkout(directory.path(), "web", "git@github.com:Acme/Web.git");
    let settings = machine(directory.path(), &[&web], true);
    let (client, task) = github_answers(Vec::new());
    let (cancel, _) = runner_parts();
    assert!(
        person_chain::for_agent_cloud_at(&client, &settings, &quiet(&cancel))
            .unwrap()
            .is_none(),
        "a cloud that may continue without GitHub access waits for no person"
    );
    task.join().unwrap();
    assert!(
        stored::load(&sign_in_path(directory.path())).is_none(),
        "nothing is signed in for a cloud that can continue without access"
    );
}

#[test]
fn a_create_that_must_ask_signs_in_once_and_stores_the_chain() {
    let directory = tempfile::tempdir().unwrap();
    let web = checkout(directory.path(), "web", "git@github.com:Acme/Web.git");
    let settings = machine(directory.path(), &[&web], false);
    let (client, task) = github(vec![
        serde_json::json!({
            "device_code": "dc", "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device", "expires_in": 899, "interval": 1
        }),
        serde_json::json!({
            "access_token": "ghu_signed_in", "token_type": "bearer", "expires_in": 28800,
            "refresh_token": "ghr_signed_in", "refresh_token_expires_in": 15_724_800
        }),
        serde_json::json!({"id": 7, "login": "octo-cat", "name": "Octo Cat"}),
        serde_json::json!({"id": 7, "login": "octo-cat", "name": "Octo Cat"}),
    ]);
    let (cancel, events) = runner_parts();
    let seen = events.clone();
    let emit = move |event| seen.lock().unwrap().push(event);
    let runner = Runner {
        cancel: &cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    let access = person_chain::for_agent_cloud_at(&client, &settings, &runner)
        .unwrap()
        .expect("the sign-in stores the chain this cloud takes");
    task.join().unwrap();
    assert_eq!(access.user.login, "octo-cat");
    assert_eq!(access.chain.access_token.expose(), "ghu_signed_in");
    let stored = stored::load(&sign_in_path(directory.path())).expect("the chain is stored for the next cloud");
    assert_eq!(stored.login, "octo-cat");
    assert_eq!(stored.refresh_token.expose(), "ghr_signed_in");
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::GitHub(Prompt::Device { user_code, .. }) if user_code == "WDJB-MJHT")),
        "the person sees the code of the one sign-in an agent's cloud needs"
    );
}

#[test]
fn a_chain_that_github_ended_is_forgotten_so_the_create_asks_again() {
    let directory = tempfile::tempdir().unwrap();
    let web = checkout(directory.path(), "web", "git@github.com:Acme/Web.git");
    let settings = machine(directory.path(), &[&web], true);
    store_chain(directory.path(), "octo-cat");
    let (client, task) = github_answers(vec![(401, serde_json::json!({"message": "Bad credentials"}))]);
    let (cancel, _) = runner_parts();
    assert!(
        person_chain::for_agent_cloud_at(&client, &settings, &quiet(&cancel))
            .unwrap()
            .is_none(),
        "a chain GitHub no longer accepts gives no access"
    );
    task.join().unwrap();
    assert!(
        stored::load(&sign_in_path(directory.path())).is_none(),
        "GitHub's word that the chain is gone ends it, as stored.rs decides"
    );
}

#[test]
fn an_outage_is_not_githubs_word_that_the_chain_is_gone() {
    let directory = tempfile::tempdir().unwrap();
    let web = checkout(directory.path(), "web", "git@github.com:Acme/Web.git");
    let settings = machine(directory.path(), &[&web], true);
    store_chain(directory.path(), "octo-cat");
    let (client, task) = github_answers(vec![(503, serde_json::json!({"message": "unavailable"}))]);
    let (cancel, _) = runner_parts();
    let refused = person_chain::for_agent_cloud_at(&client, &settings, &quiet(&cancel)).unwrap_err();
    assert!(matches!(refused, PersonChainRefusal::Kept(_)), "{refused}");
    assert!(refused.to_string().contains("try again"), "{refused}");
    task.join().unwrap();
    assert!(
        stored::load(&sign_in_path(directory.path())).is_some(),
        "an outage is not GitHub's word that the chain is gone"
    );
}

#[test]
fn a_create_that_cannot_ask_names_the_setting_that_lets_it_continue() {
    let directory = tempfile::tempdir().unwrap();
    let web = checkout(directory.path(), "web", "git@github.com:Acme/Web.git");
    let (client, _task) = github_answers(Vec::new());
    let (cancel, _) = runner_parts();
    let mut without_app = machine(directory.path(), &[&web], false);
    without_app.github = None;
    assert_eq!(
        person_chain::for_agent_cloud_at(&client, &without_app, &quiet(&cancel)).unwrap_err(),
        PersonChainRefusal::NoApp
    );
    let refused = person_chain::for_agent_cloud_at(&client, &without_app, &quiet(&cancel)).unwrap_err();
    assert!(refused.to_string().contains("allow_github_sign_in_skip"), "{refused}");
    let mut without_repository = machine(directory.path(), &[&web], false);
    if let Some(policy) = without_repository.agent_lifecycle.as_mut() {
        policy.repositories.clear();
    }
    assert_eq!(
        person_chain::for_agent_cloud_at(&client, &without_repository, &quiet(&cancel)).unwrap_err(),
        PersonChainRefusal::NoPolicyRepository
    );
    let refused = person_chain::for_agent_cloud_at(&client, &without_repository, &quiet(&cancel)).unwrap_err();
    assert!(refused.to_string().contains("allow_github_sign_in_skip"), "{refused}");
}

#[test]
fn a_commit_is_authored_as_the_person_and_not_as_a_bot() {
    let person = User {
        id: 7,
        login: "octo-cat".into(),
        name: "Octo Cat".into(),
        email: "7+octo-cat@users.noreply.github.com".into(),
    };
    let named = person_chain::author(&person);
    assert_eq!(named.login, "octo-cat");
    assert_eq!(named.name, "Octo Cat");
    assert_eq!(named.email, "7+octo-cat@users.noreply.github.com");
    let unnamed = person_chain::author(&User {
        name: String::new(),
        ..person.clone()
    });
    assert_eq!(unnamed.name, "octo-cat", "the login stands in for no name");
    assert_eq!(unnamed.email, "7+octo-cat@users.noreply.github.com");
    let unaddressed = person_chain::author(&User {
        email: String::new(),
        ..person.clone()
    });
    assert_eq!(
        unaddressed.email, "7+octo-cat@users.noreply.github.com",
        "the noreply address is built from the account whose ID it begins with"
    );
    assert_eq!(unaddressed.name, "Octo Cat");
    let public = person_chain::author(&User {
        email: "octo@example.com".into(),
        ..person.clone()
    });
    assert_eq!(
        public.email, "7+octo-cat@users.noreply.github.com",
        "a public address never goes into a commit; the noreply address attributes it either way"
    );
}
