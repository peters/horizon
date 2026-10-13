//! A rebuild that changes the cloud's settings: the changes are listed, journaled with
//! the replacement and bound only once the worker runs the new image. A cancelled,
//! refused or unconfirmed switch keeps the old profile everywhere it is recorded.
use super::*;
use horizon_cloud::{Agent, BrowserEngine};
use std::process::Command;

/// A ready cloud with agent `claude` and Chromium, and a committed profile that adds
/// `codex`, swaps Chromium for Firefox, adds the desktop, a longer readiness timeout, an
/// idle stop and another Dockerfile.
fn settings_change() -> (Fixture, Profile, Profile) {
    let fixture = Fixture::with(&json!({"agents":["claude"],"browsers":["chromium"]}));
    let bound = fixture.state().profile;
    let mut committed = bound.clone();
    committed.capabilities.agents.insert(Agent::Codex);
    committed.capabilities.browsers = [BrowserEngine::Firefox].into();
    committed.capabilities.desktop = true;
    committed.bootstrap.readiness_seconds = 1800;
    committed.idle_stop_minutes = Some(30);
    committed.build.as_mut().unwrap().dockerfile = "Dockerfile.agents".into();
    (fixture, bound, committed)
}

const CHANGES: [&str; 7] = [
    "Adds agent codex",
    "Adds browser firefox",
    "Removes browser chromium",
    "Adds the desktop",
    "Readiness timeout 600 s → 1800 s",
    "Idle stop off → 30 min",
    "Build dockerfile Dockerfile → Dockerfile.agents",
];

/// Where the cloud's profile is recorded: the deployment, its worker specification and
/// the storage journal's worker.
fn recorded(fixture: &Fixture) -> [Profile; 3] {
    let state = fixture.state();
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("workspace-volume.json")).unwrap()).unwrap();
    [
        state.profile,
        state.spec.unwrap().profile,
        serde_json::from_value(journal["worker"]["profile"].clone()).unwrap(),
    ]
}

fn script_for(fixture: &Fixture, committed: &Profile) -> Script {
    let mut script = Script::new(fixture);
    script.head.config = Some(config(committed));
    script
}

#[test]
fn each_change_is_listed_in_a_plain_sentence() {
    let (_fixture, bound, committed) = settings_change();
    assert_eq!(compatibility::changes(&bound, &committed), CHANGES);
    let mut removed = committed.clone();
    removed.capabilities.agents.clear();
    removed.capabilities.desktop = false;
    removed.capabilities.agent_approvals = horizon_cloud::AgentApprovals::Ask;
    removed.idle_stop_minutes = Some(60);
    assert_eq!(
        compatibility::changes(&committed, &removed),
        [
            "Removes agent codex",
            "Removes agent claude",
            "Removes the desktop",
            "Agent approvals auto → ask",
            "Idle stop 30 min → 60 min",
        ]
    );
    assert_eq!(compatibility::changes(&bound, &bound), Vec::<String>::new());
}

#[test]
fn a_rebuild_binds_the_committed_settings_only_once_the_worker_switches() {
    let (fixture, bound, committed) = settings_change();
    let script = script_for(&fixture, &committed);
    script.fail_at.set(Some(Boundary::Patched));
    assert_eq!(rebuild_with(&fixture, &script).unwrap_err().to_string(), CRASH);
    // Journaled with the replacement; the cloud stays bound to its old profile meanwhile.
    let state = fixture.state();
    let journal = state.image_replacement.as_ref().unwrap();
    assert_eq!(journal.profile.as_deref(), Some(&committed));
    assert_eq!(state.replacement_profile(), &committed);
    assert_eq!(recorded(&fixture), [bound.clone(), bound.clone(), bound]);
    script.fail_at.set(None);
    recover(&fixture, &script, Recovery::Continue).unwrap();
    assert_eq!(script.count("replace Next"), 1, "the worker switches once");
    assert_eq!(recorded(&fixture), [committed.clone(), committed.clone(), committed]);
    let state = fixture.state();
    assert!(state.image_replacement.is_none());
    assert_eq!(state.spec.unwrap().image_digest, digest('b'));
}

#[test]
fn a_reconnect_settles_a_switched_worker_onto_the_committed_settings() {
    let (fixture, _bound, committed) = settings_change();
    let script = script_for(&fixture, &committed);
    // The interrupted commit already rebound the storage journal to the new worker.
    script.fail_at.set(Some(Boundary::Rebound));
    assert_eq!(rebuild_with(&fixture, &script).unwrap_err().to_string(), CRASH);
    script.fail_at.set(None);
    recover(&fixture, &script, Recovery::Settle).unwrap();
    assert_eq!(recorded(&fixture), [committed.clone(), committed.clone(), committed]);
}

#[test]
fn a_cancelled_settings_change_keeps_the_old_profile_everywhere() {
    for boundary in [
        Boundary::Begun,
        Boundary::Built,
        Boundary::Requested,
        Boundary::Patched,
        Boundary::Observed,
        Boundary::Rebound,
    ] {
        let (fixture, bound, committed) = settings_change();
        let script = script_for(&fixture, &committed);
        script.fail_at.set(Some(boundary));
        assert_eq!(rebuild_with(&fixture, &script).unwrap_err().to_string(), CRASH);
        script.fail_at.set(None);
        recover(&fixture, &script, Recovery::Cancel).unwrap();
        let state = fixture.state();
        assert!(state.image_replacement.is_none(), "{boundary:?}");
        assert_eq!(
            recorded(&fixture),
            [bound.clone(), bound.clone(), bound.clone()],
            "{boundary:?}"
        );
        assert_eq!(state.spec.unwrap().image_digest, digest('a'), "{boundary:?}");
    }
}

#[test]
fn changed_settings_need_a_new_image_to_reach_the_worker() {
    let (fixture, bound, committed) = settings_change();
    let mut script = script_for(&fixture, &committed);
    script.built = digest('a');
    let error = rebuild_with(&fixture, &script).unwrap_err().to_string();
    assert_eq!(error, SAME_IMAGE);
    assert!(fixture.state().image_replacement.is_none());
    assert_eq!(recorded(&fixture), [bound.clone(), bound.clone(), bound]);
    assert_eq!(script.count("replace Next"), 0);
}

#[test]
fn an_unconfirmed_profile_or_an_unusable_credential_of_an_added_agent_is_refused() {
    let (fixture, bound, committed) = settings_change();
    let key = fixture.root.join("openai-key");
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file": "/absent", "ssh_identity_file": "/absent", "docker_config": "/absent",
        "cpu_flavors": [], "gpu_types": [], "openai_api_key_file": key
    }))
    .unwrap();
    // Codex's key file is checked only once a rebuild adds Codex.
    assert!(accept(&bound, None, &settings).is_ok());
    assert!(accept(&committed, None, &settings).is_err());
    std::fs::write(&key, "synthetic-openai-key").unwrap();
    std::fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    accept(&committed, Some(&committed), &settings).unwrap();
    let error = accept(&committed, Some(&bound), &settings).unwrap_err();
    assert_eq!(error.to_string(), CONFIRMED_OTHER);
}

#[test]
fn sessions_of_an_agent_the_rebuild_removed_end_and_the_rest_relaunch() {
    let fixture = Fixture::with(&json!({"agents":["claude"]}));
    fixture.edit(|state| {
        for (id, agent) in [("agent2", "shell"), ("agent3", "claude")] {
            state.sessions.push(Session {
                panel_id: id.into(),
                agent: agent.into(),
                tmux: id.into(),
                branch: format!("agent/{id}"),
                worktree: format!("/workspace/agents/{id}"),
            });
        }
    });
    for (capabilities, relaunched, ended) in [
        (true, vec!["agent2", "agent3"], vec!["agent1"]),
        // An image that does not check capabilities starts every agent, as before.
        (false, vec!["agent1", "agent2", "agent3"], vec![]),
    ] {
        fixture.edit(|state| state.session_restart = Some(OperationId::generate()));
        let contract = WorkerContract {
            session_restart: true,
            capabilities,
            ..WorkerContract::default()
        };
        let store = fixture.store();
        let mut state = store.load().unwrap().unwrap();
        let output = RefCell::new(Vec::new());
        let mut commands = Vec::new();
        relaunch_sessions(
            &store,
            &mut state,
            &contract,
            &|event| {
                if let Event::Output(line) = event {
                    output.borrow_mut().push(line);
                }
            },
            |command| {
                commands.push(command.split(' ').nth(3).unwrap().to_owned());
                Ok(format!("{RELAUNCH_STATUS}0\n"))
            },
        )
        .unwrap();
        assert_eq!(commands, relaunched);
        let reported: Vec<_> = ended
            .iter()
            .map(|id| {
                format!("Session {id} (codex): this cloud's profile no longer has agent codex, so its process was not relaunched")
            })
            .collect();
        assert_eq!(*output.borrow(), reported);
        assert!(store.load().unwrap().unwrap().session_restart.is_none());
    }
}

fn git(path: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.invalid"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn a_plan_reads_the_latest_commit_and_lists_its_changes() {
    let (_fixture, bound, committed) = settings_change();
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "--quiet"]);
    let commit = |profile: &Profile| {
        std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
        // JSON is YAML too.
        let yaml = serde_json::to_string(&config(profile)).unwrap();
        std::fs::write(repository.path().join(".horizon/cloud.yml"), yaml).unwrap();
        git(repository.path(), &["add", "."]);
        git(repository.path(), &["commit", "--quiet", "-m", "Cloud settings"]);
    };
    commit(&committed);
    let cancel = Cancellation::default();
    let planned = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    assert_eq!(planned.profile, committed);
    assert_eq!(planned.changes, CHANGES);
    // Uncommitted settings are not read.
    std::fs::write(repository.path().join(".horizon/cloud.yml"), "not: yaml: [").unwrap();
    assert_eq!(plan(repository.path(), &bound, "dev", &cancel).unwrap(), planned);
    let mut gpu = committed;
    gpu.gpu = true;
    commit(&gpu);
    let error = plan(repository.path(), &bound, "dev", &cancel).unwrap_err();
    assert!(error.to_string().contains("in gpu."), "{error}");
}

#[test]
fn continuing_an_unsent_settings_change_checks_the_added_agent_credential_again() {
    let (fixture, bound, committed) = settings_change();
    let script = script_for(&fixture, &committed);
    script.fail_at.set(Some(Boundary::Begun));
    assert_eq!(rebuild_with(&fixture, &script).unwrap_err().to_string(), CRASH);
    // The key file became readable by others after the rebuild was journaled.
    let key = fixture.root.join("openai-key");
    std::fs::write(&key, "synthetic-openai-key").unwrap();
    std::fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file": "/absent", "ssh_identity_file": "/absent", "docker_config": "/absent",
        "cpu_flavors": [], "gpu_types": [], "openai_api_key_file": key
    }))
    .unwrap();
    let state = fixture.state();
    let request = Request::new(
        state.cloud_id.clone(),
        state.repository.clone(),
        state.revision.clone(),
        bound,
        fixture.root.clone(),
        settings,
    );
    let error = continue_replacement(&request, &Cancellation::default(), &|_| {}).unwrap_err();
    assert_eq!(error.to_string(), "API-key file must be private (0600)");
    assert!(fixture.state().image_replacement.is_some(), "the journal stays");
}
