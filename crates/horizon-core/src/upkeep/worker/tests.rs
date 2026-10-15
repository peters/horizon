use super::*;

pub(super) fn config() -> Config {
    Config {
        repositories: vec!["acme/web".into(), "acme/api".into()],
        authors: vec!["app/dependabot".into()],
        agent: "codex".into(),
        every_minutes: 60,
        instructions: "Follow AGENTS.md.".into(),
        worker: "Hetzner cx23".into(),
        checkouts: Checkouts::OnDemand,
        order: Vec::new(),
        workflows: Vec::new(),
    }
}

#[test]
fn repositories_are_added_once_and_removed_ignoring_case() {
    let mut config = config();
    assert_eq!(
        config.add(&["Acme/Web".into(), "acme/docs".into(), "acme/docs".into()]),
        1
    );
    assert_eq!(config.repositories, ["acme/web", "acme/api", "acme/docs"]);
    config.order = vec!["acme/api".into()];
    assert_eq!(config.remove(&["ACME/API".into(), "acme/missing".into()]), 1);
    assert!(config.order.is_empty());
    assert_eq!(config.repositories, ["acme/web", "acme/docs"]);
}

#[test]
fn the_first_repository_starts_the_cloud_and_clones_follow_the_choice() {
    let mut config = config();
    assert_eq!(config.primary(), Some("acme/web"));
    assert!(prompt(&config).contains("Clone only when a pull request needs a fix"));
    config.checkouts = Checkouts::Full;
    assert!(prompt(&config).contains("git lfs pull"));
    assert_eq!(Checkouts::from_name("partial"), Some(Checkouts::Partial));
}

#[test]
fn the_command_refuses_names_that_could_escape_the_session() {
    let revision = "a".repeat(40);
    assert!(command("abc-123", &revision, &config(), Signals::default()).is_ok());
    assert!(command("abc;rm", &revision, &config(), Signals::default()).is_err());
    assert!(command("abc", "HEAD", &config(), Signals::default()).is_err());
}

#[test]
fn settings_that_could_reach_a_command_line_or_path_are_refused() {
    assert!(config().validate().is_ok());
    let refused = |change: fn(&mut Config)| {
        let mut config = config();
        change(&mut config);
        config.validate().is_err()
    };
    assert!(refused(|config| config.repositories.clear()));
    assert!(refused(|config| config.repositories.push("--repo=evil/x".into())));
    assert!(refused(|config| config.repositories.push("acme/..".into())));
    assert!(refused(|config| config.repositories.push("acme/web/../x".into())));
    assert!(refused(|config| config.repositories.push("acme/a b".into())));
    assert!(refused(|config| config.repositories.push("ACME/Web".into())));
    assert!(refused(
        |config| config.repositories = (0..=REPOSITORY_LIMIT).map(|n| format!("acme/r{n}")).collect()
    ));
    assert!(refused(|config| config.order.push("acme/other".into())));
    assert!(refused(|config| config.authors.clear()));
    assert!(refused(|config| config.authors.push("--state=closed".into())));
    assert!(refused(|config| config.authors.push("app/".into())));
    assert!(refused(|config| config.authors.push("bot;id".into())));
    assert!(refused(|config| config.authors.push("app/dependabot[bot]".into())));
    assert!(refused(|config| config.agent = "claude".into()));
    assert!(refused(|config| config.every_minutes = 5));
    assert!(refused(
        |config| config.instructions = "x".repeat(INSTRUCTIONS_LIMIT + 1)
    ));
    assert!(refused(|config| config.worker = "x".repeat(WORKER_LIMIT + 1)));
    assert!(refused(|config| config.instructions = "Merge.\u{1b}[2J".into()));
    assert!(refused(|config| config.worker = "Hetzner\n".into()));
    let mut config = config();
    config.authors = vec!["app/dependabot".into(), "renovate[bot]".into(), "dependabot".into()];
    config.order = vec!["acme/api".into()];
    assert!(config.validate().is_ok());
}

#[test]
fn the_command_runs_as_the_agent_where_the_worker_isolates_it() {
    let remote = command("abc-123", &"a".repeat(40), &config(), Signals::default()).unwrap();
    // The worker procedure in docs/testing/procedures/upkeep.md runs this exact command.
    if let Some(path) = std::env::var_os("HORIZON_TEST_UPKEEP_COMMAND") {
        std::fs::write(path, &remote).unwrap();
    }
    assert!(remote.starts_with(
        "launcher=python3; if [ -f /run/horizon-tailnet/agent-isolation ]; then launcher='horizon-worker-tailnet agent python3'; fi;"
    ));
    assert!(remote.contains("| $launcher -I - '"));
    // The longest settings Config::validate accepts, in the characters that escaping makes
    // longest, still fit the worker's shell.
    let mut long = config();
    long.instructions = "\"".repeat(INSTRUCTIONS_LIMIT);
    long.worker = "\\".repeat(WORKER_LIMIT);
    long.repositories = (0..REPOSITORY_LIMIT)
        .map(|n| format!("{}/{n:0>100}", "o".repeat(39)))
        .collect();
    long.order.clone_from(&long.repositories);
    long.authors = (0..AUTHOR_LIMIT).map(|n| format!("{n:0>39}[bot]")).collect();
    // The workflows travel in the policy, not in this command.
    long.workflows = (0..policy::WORKFLOW_LIMIT)
        .map(|n| policy::Workflow {
            repository: long.repositories[n].clone(),
            workflow: format!("{n:0>96}.yml"),
        })
        .collect();
    assert!(long.validate().is_ok());
    let longest = command("abc-123", &"a".repeat(40), &long, Signals::default()).unwrap();
    // Room for the scripts to grow before this limit is a concern.
    assert!(longest.len() < COMMAND_LIMIT - 12 * 1024, "{}", longest.len());
    assert!(command("abc-123", &"A".repeat(40), &config(), Signals::default()).is_err());
}

#[test]
fn a_cloud_id_that_is_not_one_folder_name_is_never_saved() {
    assert!(save("../escape", &config()).is_err());
    assert!(save("", &config()).is_err());
}

#[test]
fn the_default_instructions_lead_the_prompt() {
    let mut config = config();
    config.instructions = crate::upkeep::DEFAULT_INSTRUCTIONS.into();
    let prompt = prompt(&config);
    assert!(prompt.starts_with("Keep dependency pull requests moving"));
    assert!(prompt.contains("Pull requests opened by: app/dependabot."));
    assert!(prompt.contains("--match-head-commit"));
}

#[test]
fn the_prompt_is_a_template_for_one_repository() {
    let prompt = prompt(&config());
    assert!(prompt.starts_with("Follow AGENTS.md."));
    assert!(prompt.contains("Repository: @@REPOSITORY@@."));
    assert!(prompt.contains("the 1 other repositories"));
    assert!(prompt.contains("write @@STATUS@@ again"));
    assert!(prompt.contains("github_access"));
}

#[test]
fn the_document_keeps_the_last_record_of_each_run() {
    let exchange = parse(
        r#"{"loop": true, "paused": false, "next_run": 1800000000,
            "status": "{\"pull_requests\": [{\"repository\": \"acme/web\"}]}",
            "runs": [{"started_at": 1799996400, "outcome": "running"},
                     {"started_at": 1799996400, "finished_at": 1799997000, "outcome": "done", "merged": 2}]}"#,
    )
    .unwrap();
    let document = exchange.document(&config());
    assert_eq!(document["runs"].as_array().unwrap().len(), 1);
    assert_eq!(document["runs"][0]["outcome"], "done");
    assert_eq!(document["runs"][0]["merged"], 2);
    assert_eq!(document["pull_requests"][0]["repository"], "acme/web");
    assert_eq!(document["schedule"]["every_minutes"], 60);
}

#[test]
fn refusals_that_root_logged_join_the_activity_of_the_agent() {
    let exchange = parse(
        r#"{"loop": true, "runs": [], "next_run": null,
            "status": "{\"activity\": [{\"at\": \"2026-10-14T10:00:00Z\", \"text\": \"Merged #1\"}]}",
            "refused": [{"at": 1791972000, "text": "The worker refused a merge on acme/web: #7 is by someone."},
                        {"at": "soon", "text": "left out"}, {"at": 1791972000}]}"#,
    )
    .unwrap();
    let activity = exchange.document(&config())["activity"].clone();
    assert_eq!(
        activity,
        serde_json::json!([
            {"at": "2026-10-14T10:00:00Z", "text": "Merged #1"},
            {"at": "2026-10-14T10:00:00Z", "text": "The worker refused a merge on acme/web: #7 is by someone."}
        ])
    );
    // An agent that writes no activity, or a list of the wrong type, still leaves the refusals.
    let exchange = parse(
        r#"{"loop": true, "runs": [], "next_run": null, "status": "{\"activity\": 7}",
            "refused": [{"at": 1791972000, "text": "The worker refused a merge on acme/web."}]}"#,
    )
    .unwrap();
    assert_eq!(exchange.document(&config())["activity"].as_array().unwrap().len(), 1);
}

#[test]
fn a_report_that_is_not_json_is_left_out() {
    let exchange = parse(r#"{"loop": false, "status": "{half", "runs": [], "next_run": null}"#).unwrap();
    assert_eq!(exchange.document(&config())["pull_requests"], serde_json::json!([]));
}

#[test]
fn a_run_now_travels_with_its_token() {
    let payload = |signals: Signals| -> serde_json::Value {
        let remote = command("abc-123", &"a".repeat(40), &config(), signals).unwrap();
        let encoded = remote.rsplit(" - '").next().unwrap().trim_end_matches('\'');
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    };
    assert_eq!(payload(Signals::default())["run_now"], false);
    let run = Signals {
        run_now: true,
        run_token: Some(0xab),
        ..Signals::default()
    };
    assert_eq!(payload(run)["run_now"], format!("{:032x}", 0xab));
    let token = Signals {
        run_now: true,
        ..Signals::default()
    }
    .with_run_token()
    .run_token;
    assert!(token.is_some());
    assert_eq!(
        Signals::default().with_run_token().run_token,
        None,
        "no Run now, no token"
    );
}

#[test]
fn a_board_belongs_to_the_upkeep_whose_folder_holds_its_status() {
    let status = directory("c1").join("status.json");
    assert_eq!(cloud_of(&status).as_deref(), Some("c1"));
    assert_eq!(cloud_of(&directory("c1").join("other.json")), None);
    assert_eq!(
        cloud_of(std::path::Path::new("/tmp/c1/status.json")),
        None,
        "another folder"
    );
    assert_eq!(
        cloud_of(&super::super::default_status_path()),
        None,
        "the default board"
    );
}

#[test]
fn an_update_refuses_a_cloud_that_is_not_one_folder() {
    assert!(update("../c1", |_| {}).is_err());
}

#[test]
fn moving_a_first_run_marker_needs_one_and_a_plain_cloud() {
    assert!(first_run_moved("../c1", 1).is_err());
    let cloud = "upkeep-test-no-marker";
    first_run_moved(cloud, 1).unwrap();
    assert_eq!(first_run_pending(cloud).unwrap(), None, "no marker is made");
}
