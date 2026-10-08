use super::{
    Endpoint,
    portfolio::{
        Counts, Filter, PrStage, RepositoryStatus, Tone, ago, counts, github_pull_request_url, health, keep_selection,
        pr_stages, reported_health, reported_heartbeat, repositories,
    },
    setup::{GitHubStep, RepositoriesStep, Setup, WorkerProbe, WorkerStep},
    transport::{diagnostic_summary, parse_status},
};
use serde_json::{Value, json};

fn portfolio() -> Value {
    json!({"schema_version": 1, "repos":[
        {"repository":"sample/web", "ecosystems":["npm"], "status":"blocked"},
        {"repository":"sample/api", "ecosystems":["nuget"], "status":"active"},
        {"repository":"sample/native", "ecosystems":["cargo"], "status":"queued"},
        {"repository":"sample/finished", "ecosystems":["pip"], "status":"complete"},
        {"repository":"sample/idle", "ecosystems":["gomod"], "status":"idle"},
        {"repository":"sample/paused", "ecosystems":["npm"], "status":"queued", "enabled":false}],
        "prs":[
            {"repository":"sample/web", "status":"CI failed", "title":"Update frontend group"},
            {"repository":"sample/api", "status":"Working"},
            {"repository":"sample/native", "status":"Queued"},
            {"repository":"sample/finished", "status":"Verified"},
            {"repository":"sample/paused", "status":"Queued"}]})
}

fn visible(snapshot: &Value, filter: Filter, search: &str) -> Vec<String> {
    repositories(snapshot)
        .iter()
        .filter(|repo| repo.matches(filter, search))
        .map(|repo| repo.name.to_owned())
        .collect()
}

#[test]
fn portfolio_counts_repositories_and_open_prs_from_snapshot() {
    let snapshot = portfolio();
    let repositories = repositories(&snapshot);
    assert_eq!(
        counts(&repositories),
        Counts {
            open_prs: 4,
            attention: 1,
            active: 1,
            queued: 1,
            complete: 1
        }
    );
    assert_eq!(repositories[4].status, RepositoryStatus::Idle);
    assert_eq!(repositories[5].status, RepositoryStatus::Disabled);
}

#[test]
fn pull_request_pipeline_counts_every_stage_once() {
    assert_eq!(pr_stages(&portfolio()), [1, 1, 2, 0, 1, 0]);
    for (status, stage) in [
        ("Verified", PrStage::Verified),
        ("Running CI", PrStage::Working),
        ("Fixing CI", PrStage::Working),
        ("Queued", PrStage::Queued),
        ("Blocked", PrStage::Blocked),
        ("CI failed", PrStage::Failed),
        ("Disabled", PrStage::Paused),
    ] {
        assert_eq!(PrStage::of(&json!({ "status": status })), stage, "{status}");
    }
    assert_eq!(
        PrStage::of(&json!({"state":"merged", "status":"Queued"})),
        PrStage::Verified
    );
}

#[test]
fn filters_combine_status_search_and_pull_request_title() {
    let snapshot = portfolio();
    assert_eq!(visible(&snapshot, Filter::Attention, "  FRONTEND "), ["sample/web"]);
    assert_eq!(visible(&snapshot, Filter::All, "nuget"), ["sample/api"]);
    assert_eq!(visible(&snapshot, Filter::Complete, ""), ["sample/finished"]);
    assert_eq!(visible(&snapshot, Filter::All, "").len(), 6);
}

#[test]
fn selection_survives_status_updates_but_follows_the_persons_search() {
    let mut snapshot = portfolio();
    snapshot["repos"][2]["status"] = json!("complete");
    let updated = repositories(&snapshot);
    let shown = |filter: Filter, search: &str| {
        updated
            .iter()
            .filter(|repo| repo.matches(filter, search))
            .collect::<Vec<_>>()
    };
    assert!(keep_selection(
        Some("sample/native"),
        &updated,
        &shown(Filter::Queued, ""),
        false
    ));
    assert!(!keep_selection(
        Some("sample/native"),
        &updated,
        &shown(Filter::Attention, ""),
        true
    ));
    assert!(!keep_selection(
        Some("sample/native"),
        &updated,
        &shown(Filter::All, "does not exist"),
        true
    ));
    assert!(!keep_selection(
        Some("removed/repository"),
        &updated,
        &shown(Filter::All, ""),
        false
    ));
}

#[test]
fn worker_health_uses_process_liveness_and_heartbeat() {
    let label = |worker: Value| health(&worker).label;
    assert_eq!(
        label(json!({"alive":true,"state":"idle","heartbeat_age_seconds":2})),
        "Agent online · idle"
    );
    assert_eq!(
        label(json!({"alive":true,"state":"working","heartbeat_age_seconds":2})),
        "Agent online · working"
    );
    assert_eq!(
        label(json!({"alive":true,"state":"working","heartbeat_age_seconds":20})),
        "Agent unresponsive"
    );
    assert_eq!(label(json!({"alive":false,"state":"stopped"})), "Agent stopped");
    assert_eq!(health(&json!({"state":"error"})).tone, Tone::Warning);
}

#[test]
fn ssh_disconnect_hides_cached_green_worker_health() {
    let mut status = json!({"ssh_connected":false,
        "worker_health":{"alive":true,"state":"working","heartbeat_age_seconds":1}});
    assert_eq!(reported_health(&status).label, "Agent unknown · SSH unavailable");
    assert_eq!(reported_heartbeat(&status), None);
    status["ssh_connected"] = json!(true);
    assert_eq!(reported_health(&status).label, "Agent online · working");
    assert_eq!(reported_heartbeat(&status), Some(1.0));
}

#[test]
fn heartbeat_age_reads_as_words() {
    assert_eq!(ago(0.4), "just now");
    assert_eq!(ago(12.7), "12s ago");
    assert_eq!(ago(1853.0), "30 min ago");
    assert_eq!(ago(7300.0), "2 h ago");
    assert_eq!(ago(200_000.0), "2 d ago");
}

#[test]
fn external_link_must_match_reported_repository_and_pull_request() {
    let mut pr =
        json!({"repository":"sample-org/web", "number":124, "url":"https://github.com/sample-org/web/pull/124"});
    assert_eq!(github_pull_request_url(&pr), pr["url"].as_str());
    for url in [
        "file:///tmp/worker",
        "https://github.com.evil.test/sample-org/web/pull/124",
        "https://github.com/another-org/web/pull/124",
        "https://github.com/sample-org/web/pull/125",
        "https://github.com/sample-org/web/pull/124/commits",
        "https://github.com/sample-org/web/pull/124?token=secret",
        "https://github.com/sample-org/../pull/124",
    ] {
        pr["url"] = json!(url);
        assert_eq!(github_pull_request_url(&pr), None, "{url}");
    }
}

#[test]
fn nothing_after_github_unlocks_until_github_is_connected() {
    let real = json!({"schema_version":1, "repos":[{}, {}]});
    for probe in [
        WorkerProbe::Unavailable,
        WorkerProbe::Connecting,
        WorkerProbe::Unreachable("down"),
        WorkerProbe::Reporting(&real),
    ] {
        let setup = Setup::evaluate(false, probe);
        assert_eq!(setup.github, GitHubStep::Needed);
        assert_eq!(setup.repositories, RepositoriesStep::Blocked);
        assert_eq!(setup.worker, WorkerStep::Blocked);
        assert!(!setup.complete());
        assert_eq!(setup.current(), 1);
    }
}

#[test]
fn connected_github_opens_the_portfolio_once_a_worker_reports() {
    let real = json!({"schema_version":1, "repos":[{}, {}]});
    let waiting = Setup::evaluate(true, WorkerProbe::Unavailable);
    assert_eq!(waiting.github, GitHubStep::Connected);
    assert_eq!(waiting.repositories, RepositoriesStep::Choose);
    assert_eq!(waiting.worker, WorkerStep::Unavailable);
    assert_eq!(waiting.current(), 2);
    assert!(!waiting.complete());
    let running = Setup::evaluate(true, WorkerProbe::Reporting(&real));
    assert_eq!(
        running.repositories,
        RepositoriesStep::Reported {
            count: 2,
            simulated: false
        }
    );
    assert_eq!(running.worker, WorkerStep::Running { simulated: false });
    assert!(running.complete());
}

#[test]
fn only_a_synthetic_test_worker_may_stand_in_for_github() {
    let synthetic = json!({"schema_version":1, "synthetic":true, "repos":[{}]});
    let setup = Setup::evaluate(false, WorkerProbe::Reporting(&synthetic));
    assert_eq!(setup.github, GitHubStep::Simulated);
    assert_eq!(setup.worker, WorkerStep::Running { simulated: true });
    assert!(setup.complete());
    let claimed = json!({"schema_version":1, "synthetic":"yes", "repos":[{}]});
    assert_eq!(
        Setup::evaluate(false, WorkerProbe::Reporting(&claimed)).github,
        GitHubStep::Needed
    );
}

#[test]
fn status_must_use_the_known_schema_and_test_workers_must_say_synthetic() {
    assert!(parse_status(br#"{"schema_version":1,"synthetic":true}"#, true).is_ok());
    assert!(parse_status(br#"{"schema_version":1}"#, false).is_ok());
    assert!(parse_status(br#"{"schema_version":1}"#, true).is_err());
    assert!(parse_status(br#"{"schema_version":2,"synthetic":true}"#, true).is_err());
    assert!(parse_status(b"[1]", false).is_err());
    assert!(parse_status(b"not json", false).is_err());
}

#[test]
fn test_worker_must_be_synthetic_strict_loopback_ssh() {
    let root = tempfile::tempdir().unwrap();
    let write = |value: Value| std::fs::write(root.path().join("connection.json"), value.to_string()).unwrap();
    let good =
        json!({"synthetic":true,"host":"127.0.0.1","username":"worker","host_key_checking":"strict","port":2222});
    write(good.clone());
    let endpoint = Endpoint::fixture(root.path()).unwrap();
    assert!(endpoint.synthetic());
    assert_eq!(endpoint.address(), "worker@127.0.0.1:2222");
    let arguments = endpoint.terminal_arguments();
    assert_eq!(arguments.first().map(String::as_str), Some("-tt"));
    assert_eq!(arguments.last().map(String::as_str), Some("maintenance run"));
    assert!(arguments.iter().any(|argument| argument == "StrictHostKeyChecking=yes"));
    for (field, value) in [
        ("synthetic", json!(false)),
        ("host", json!("example.invalid")),
        ("username", json!("root")),
        ("host_key_checking", json!("accept-new")),
        ("port", json!(0)),
        ("port", json!(70_000)),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        write(bad);
        assert!(Endpoint::fixture(root.path()).is_err(), "{field}");
    }
}

#[test]
fn diagnostic_context_omits_prompts_logs_and_untrusted_commands() {
    let value = json!({"synthetic":true,"status":{"worker_health":{"alive":false,"state":"error","last_error":"private value"},"configured_revision":2,"applied_revision":1},"policy":{"global_prompt":"secret prompt"},"log_tail":"private log","command":"unexpected command"});
    let summary = diagnostic_summary(&value, true).unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("connection.json"),
        json!({"synthetic":true,"host":"127.0.0.1","username":"worker","host_key_checking":"strict","port":2222})
            .to_string(),
    )
    .unwrap();
    let prompt = Endpoint::fixture(root.path()).unwrap().debug_prompt(&summary);
    for private in ["private value", "secret prompt", "private log", "unexpected command"] {
        assert!(!prompt.contains(private), "{private}");
    }
    assert!(prompt.contains("configured_revision"));
    assert!(prompt.contains("maintenance diagnose"));
    assert!(diagnostic_summary(&json!({"synthetic":false}), true).is_err());
}
