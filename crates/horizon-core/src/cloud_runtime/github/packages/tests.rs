use super::*;
use horizon_cloud::github::Secret;

fn sample(token: &str) -> Chain {
    let now = std::time::SystemTime::now();
    Chain {
        access_token: Secret::new(token.into()),
        access_expires_at: now + Duration::from_hours(8),
        refresh_token: Secret::new(format!("{token}-refresh")),
        refresh_expires_at: now + Duration::from_hours(180 * 24),
    }
}

#[test]
fn the_worker_status_says_what_it_holds_and_whether_github_works() {
    let line = |state: &str, extra: &str| {
        format!(
            "{{\"state\":\"{state}\",\"login\":\"octo\",\"repositories\":[{{\"repository\":\"acme/api\",\"target\":\"primary\"}}]{extra}}}"
        )
    };
    assert_eq!(held(&line("ok", ",\"packages\":null")), (Held::Missing, true));
    assert_eq!(
        held(&line(
            "ok",
            ",\"packages\":{\"state\":\"ok\",\"repositories\":[\"B/x\",\"a/y\"],\"repositories_change\":true}"
        )),
        (
            Held::Current {
                repositories: vec!["a/y".into(), "b/x".into()],
                change: true
            },
            true
        )
    );
    let old = held(&line(
        "ok",
        ",\"packages\":{\"state\":\"ok\",\"repositories\":[\"a/y\"]}",
    ));
    assert!(matches!(old.0, Held::Current { change: false, .. }));
    assert_eq!(
        held(&line("ok", ",\"packages\":{\"state\":\"revoked\"}")).0,
        Held::Missing
    );
    // The GitHub step's own reading: an absent chain, a stopped service or no repository is no access.
    assert_eq!(held(&line("absent", ",\"packages\":null")), (Held::Missing, false));
    assert!(!held(&line("ok", ",\"packages\":null,\"serving\":false")).1);
    assert_eq!(
        held("{\"state\":\"ok\",\"repositories\":[],\"packages\":null}"),
        (Held::Missing, false)
    );
    assert_eq!(held(&line("ok", "")).0, Held::Unsupported);
    assert_eq!(held("not json"), (Held::Unsupported, false));
    // What STATUS prints for an image without the service; a failed status is no output at all.
    assert_eq!(held("{\"state\":\"unsupported\"}"), (Held::Unsupported, false));
    assert!(STATUS.contains("command -v horizon-worker-github") && !STATUS.contains("|| true"));
}

#[test]
fn only_the_upkeep_repositories_go_with_the_chain_once_each_and_at_most_fifty() {
    let config = |repositories: Vec<String>| crate::upkeep::worker::Config {
        repositories,
        ..crate::upkeep::worker::Config::default()
    };
    let names = |list: &[&str]| list.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
    assert_eq!(
        own_repositories(&config(names(&["Acme/API", "acme/api", "acme/web", "not a repo"]))).unwrap(),
        ["acme/api", "acme/web"]
    );
    let many = (0..51).map(|index| format!("acme/r{index}")).collect();
    assert!(own_repositories(&config(many)).is_err());
}

#[test]
fn only_exactly_read_packages_reaches_the_worker() {
    let scopes = |list: &[&str]| list.iter().map(|scope| (*scope).to_owned()).collect::<Vec<_>>();
    assert!(check_scopes(Some(&scopes(&["read:packages"]))).is_ok());
    for list in [
        &["write:packages"][..],
        &["read:packages", "repo"],
        &[],
        &["READ:PACKAGES"],
    ] {
        assert!(
            matches!(check_scopes(Some(&scopes(list))), Err(Ended::Reason(reason)) if reason.contains("does not get")),
            "{list:?}"
        );
    }
    assert!(matches!(check_scopes(None), Err(Ended::Reason(_))));
}

#[test]
fn the_payload_holds_the_client_id_and_the_chain_but_no_secret() {
    let repositories = vec!["acme/api".to_owned(), "acme/web".to_owned()];
    // Bytes in memory (wiped on drop), never a named temporary file: the type says so.
    let request: zeroize::Zeroizing<Vec<u8>> = payload(&sample("gho_synthetic-packages"), &repositories).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&request).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["client_id"], publish::CLIENT_ID);
    assert_eq!(value["repositories"], serde_json::json!(["acme/api", "acme/web"]));
    assert_eq!(value["chain"]["access_token"], "gho_synthetic-packages");
    assert_eq!(value["chain"]["refresh_token"], "gho_synthetic-packages-refresh");
    assert!(value["chain"]["access_expires_at"].as_u64().unwrap() > 0);
    assert!(value.get("client_secret").is_none());
}
