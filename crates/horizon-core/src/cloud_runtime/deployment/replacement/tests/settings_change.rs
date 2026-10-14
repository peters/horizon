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
    let error = accept(&committed, None, &settings).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Agent codex cannot use its API-key file {}: the file is missing. A reconnect of this cloud with this profile needs this file too, so fix it before the rebuild",
            key.display()
        )
    );
    std::fs::write(&key, "synthetic-openai-key").unwrap();
    std::fs::set_permissions(&key, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    accept(&committed, Some(&committed), &settings).unwrap();
    let error = accept(&committed, Some(&bound), &settings).unwrap_err();
    assert_eq!(error.to_string(), CONFIRMED_OTHER);
}

#[test]
fn a_rebuild_that_adds_an_agent_names_the_unusable_file_of_an_agent_the_cloud_already_has() {
    let (fixture, bound, committed) = settings_change();
    let (claude, codex) = (fixture.root.join("anthropic-key"), fixture.root.join("openai-key"));
    std::fs::write(&codex, "synthetic-openai-key").unwrap();
    std::fs::set_permissions(&codex, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let settings: Settings = serde_json::from_value(json!({
        "runpod_key_file": "/absent", "ssh_identity_file": "/absent", "docker_config": "/absent",
        "cpu_flavors": [], "gpu_types": [], "anthropic_api_key_file": claude, "openai_api_key_file": codex
    }))
    .unwrap();
    // Claude's file is missing although the cloud already has Claude: the refusal says so.
    for profile in [&bound, &committed] {
        let error = accept(profile, None, &settings).unwrap_err().to_string();
        assert_eq!(
            error,
            format!(
                "Agent claude cannot use its API-key file {}: the file is missing. A reconnect of this cloud with this profile needs this file too, so fix it before the rebuild",
                claude.display()
            )
        );
    }
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
                removed_agent_reported: false,
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

/// The report of the fixture's Codex session once the profile no longer has Codex.
const ENDED: &str =
    "Session agent1 (codex): this cloud's profile no longer has agent codex, so its process was not relaunched";

/// Relaunches the sessions of `state` as a reconnect does, with each relaunch reporting
/// `status`; returns the output lines and the panels it tried to relaunch. A status that
/// is not a number fails the relaunch.
fn reconnect(store: &Store, state: &mut Deployment, status: &str) -> (Vec<String>, Vec<String>) {
    state.session_restart = Some(OperationId::generate());
    let contract = WorkerContract {
        session_restart: true,
        capabilities: true,
        ..WorkerContract::default()
    };
    let output = RefCell::new(Vec::new());
    let mut commands = Vec::new();
    let relaunched = relaunch_sessions(
        store,
        state,
        &contract,
        &|event| {
            if let Event::Output(line) = event {
                output.borrow_mut().push(line);
            }
        },
        |command| {
            commands.push(command.split(' ').nth(3).unwrap().to_owned());
            Ok(format!("{RELAUNCH_STATUS}{status}\n"))
        },
    );
    assert_eq!(relaunched.is_ok(), status.parse::<i32>().is_ok(), "{relaunched:?}");
    (output.into_inner(), commands)
}

#[test]
fn a_removed_agents_session_is_reported_once_and_relaunches_when_the_agent_returns() {
    const LOST: &str = "Session agent1 (codex) lost its process in the container reset and was not relaunched";
    let fixture = Fixture::with(&json!({"agents":["claude"]}));
    // The mark as the record on disk holds it.
    let mark = || {
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.root.join("deployment.json")).unwrap()).unwrap();
        record["sessions"][0].get("removed_agent_reported").cloned()
    };
    // A record from before the mark existed loads unmarked and saves without the field.
    assert!(!fixture.state().sessions[0].removed_agent_reported);
    assert_eq!(mark(), None);
    {
        let store = fixture.store();
        let mut state = store.load().unwrap().unwrap();
        assert_eq!(reconnect(&store, &mut state, "0"), (vec![ENDED.to_owned()], vec![]));
        assert_eq!(mark(), Some(json!(true)));
        // A second reconnect stays quiet.
        assert_eq!(reconnect(&store, &mut state, "0"), (vec![], vec![]));
    }
    // So does a reconnect after Horizon restarts and reads the record again.
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    assert!(state.sessions[0].removed_agent_reported);
    assert_eq!(reconnect(&store, &mut state, "0"), (vec![], vec![]));
    // A rebuild that adds the agent again relaunches the session and clears the mark.
    state.profile.capabilities.agents.insert(Agent::Codex);
    assert_eq!(reconnect(&store, &mut state, "0"), (vec![], vec!["agent1".to_owned()]));
    assert_eq!(mark(), None);
    // The mark never hides a session that ends for another reason.
    assert_eq!(
        reconnect(&store, &mut state, "3"),
        (vec![LOST.to_owned()], vec!["agent1".to_owned()])
    );
    // Removed again, the session is reported again, once.
    state.profile.capabilities.agents.remove(&Agent::Codex);
    assert_eq!(reconnect(&store, &mut state, "0"), (vec![ENDED.to_owned()], vec![]));
    assert_eq!(reconnect(&store, &mut state, "0"), (vec![], vec![]));
}

#[test]
fn a_relaunch_that_fails_keeps_the_report_of_a_removed_agents_session() {
    let fixture = Fixture::with(&json!({"agents":["claude"]}));
    fixture.edit(|state| {
        state.sessions.push(Session {
            panel_id: "agent2".into(),
            agent: "claude".into(),
            tmux: "agent2".into(),
            branch: "agent/agent2".into(),
            worktree: "/workspace/agents/agent2".into(),
            removed_agent_reported: false,
        });
    });
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    assert_eq!(
        reconnect(&store, &mut state, "unconfirmed"),
        (vec![ENDED.to_owned()], vec!["agent2".to_owned()])
    );
    // The retry relaunches the other session and does not report the removed one again.
    let mut state = store.load().unwrap().unwrap();
    assert!(state.session_restart.is_some() && state.sessions[0].removed_agent_reported);
    assert_eq!(reconnect(&store, &mut state, "0"), (vec![], vec!["agent2".to_owned()]));
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
fn a_plan_reads_the_working_copy_and_lists_its_changes_and_uncommitted_files() {
    let (_fixture, bound, committed) = settings_change();
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "--quiet"]);
    let settings = repository.path().join(".horizon/cloud.yml");
    // JSON is YAML too.
    let save = |profile: &Profile| {
        std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
        std::fs::write(&settings, serde_json::to_string(&config(profile)).unwrap()).unwrap();
    };
    let commit = |profile: &Profile| {
        save(profile);
        git(repository.path(), &["add", "."]);
        git(repository.path(), &["commit", "--quiet", "-m", "Cloud settings"]);
    };
    commit(&committed);
    let cancel = Cancellation::default();
    let planned = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    assert_eq!(planned.profile, committed);
    assert_eq!(planned.changes, CHANGES);
    assert!(planned.notes.is_empty() && planned.uncommitted.is_empty());

    // The checkout moved past the cloud's commit and has uncommitted settings: the plan
    // reads the latest commit with the working copy, and lists the file, which is in the
    // build context.
    let mut later = committed.clone();
    later.idle_stop_minutes = Some(45);
    commit(&later);
    let mut working = later.clone();
    working.capabilities.desktop = false;
    save(&working);
    let uncommitted = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    assert_eq!(uncommitted.profile, working);
    assert!(uncommitted.changes.contains(&"Idle stop off → 45 min".to_owned()));
    assert!(!uncommitted.changes.contains(&"Adds the desktop".to_owned()));
    let files: Vec<_> = uncommitted
        .uncommitted
        .files()
        .iter()
        .map(|file| (file.path.as_str(), file.change.label()))
        .collect();
    assert_eq!(files, [(".horizon/cloud.yml", "modified")]);

    // A working copy that does not parse leaves the committed settings, and says so.
    std::fs::write(&settings, "not: yaml: [").unwrap();
    let invalid = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    assert_eq!(invalid.profile, later);
    assert_eq!(
        invalid.notes,
        ["The working copy of .horizon/cloud.yml is not valid, so the committed settings apply."]
    );
    let mut gpu = committed;
    gpu.gpu = true;
    save(&gpu);
    let error = plan(repository.path(), &bound, "dev", &cancel).unwrap_err();
    assert!(error.to_string().contains("in gpu."), "{error}");
}

#[test]
fn uncommitted_files_must_be_the_confirmed_ones() {
    let (_fixture, bound, _) = settings_change();
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "--quiet"]);
    std::fs::create_dir_all(repository.path().join(".horizon")).unwrap();
    std::fs::write(
        repository.path().join(".horizon/cloud.yml"),
        serde_json::to_string(&config(&bound)).unwrap(),
    )
    .unwrap();
    git(repository.path(), &["add", "."]);
    git(repository.path(), &["commit", "--quiet", "-m", "Cloud settings"]);
    let cancel = Cancellation::default();
    let clean = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    confirm_files(&clean.uncommitted, None).unwrap();
    confirm_files(&clean.uncommitted, Some(&clean)).unwrap();
    std::fs::write(repository.path().join("Dockerfile"), "FROM scratch\n").unwrap();
    let dirty = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    for confirmed in [None, Some(&clean)] {
        let error = confirm_files(&dirty.uncommitted, confirmed).unwrap_err();
        assert_eq!(error.to_string(), UNCONFIRMED_FILES);
    }
    confirm_files(&dirty.uncommitted, Some(&dirty)).unwrap();
    // The token of a printed plan binds the settings and the files.
    assert_ne!(clean.token(), dirty.token());
    assert_eq!(
        dirty.token(),
        plan(repository.path(), &bound, "dev", &cancel).unwrap().token()
    );
    let mut other = dirty.clone();
    other.profile.idle_stop_minutes = Some(45);
    assert_ne!(other.token(), dirty.token());
    std::fs::write(repository.path().join("Dockerfile"), "FROM scratch\nRUN true\n").unwrap();
    let changed = plan(repository.path(), &bound, "dev", &cancel).unwrap();
    assert!(
        confirm_files(&changed.uncommitted, Some(&dirty)).is_err(),
        "same name, other content"
    );
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
    assert_eq!(
        error.to_string(),
        format!(
            "Agent codex cannot use its API-key file {}: API-key file must be private (0600). A reconnect of this cloud with this profile needs this file too, so fix it before the rebuild",
            key.display()
        )
    );
    assert!(fixture.state().image_replacement.is_some(), "the journal stays");
}

/// The uncommitted files of a synthetic checkout whose Dockerfile changed.
fn dirty_recipe() -> (tempfile::TempDir, Uncommitted) {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "--quiet"]);
    std::fs::write(repository.path().join("Dockerfile"), "FROM scratch\n").unwrap();
    git(repository.path(), &["add", "."]);
    git(repository.path(), &["commit", "--quiet", "-m", "Recipe"]);
    std::fs::write(repository.path().join("Dockerfile"), "FROM scratch\nRUN true\n").unwrap();
    let revision = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(repository.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let build = horizon_cloud::Build {
        context: ".".into(),
        dockerfile: "Dockerfile".into(),
        platform: "linux/amd64".into(),
    };
    let files =
        crate::cloud_runtime::repository::uncommitted::collect(repository.path(), revision.trim(), &build).unwrap();
    assert!(!files.is_empty());
    (repository, files)
}

#[test]
fn a_rebuild_from_uncommitted_files_journals_their_hash_and_the_card_names_it() {
    let fixture = Fixture::new();
    let mut script = Script::new(&fixture);
    let (_repository, files) = dirty_recipe();
    let hash = files.hash().unwrap();
    script.uncommitted = files;
    let store = fixture.store();
    let mut state = store.load().unwrap().unwrap();
    let recipes = latest_recipes(&script, &state, "dev", &|_| {}).unwrap();
    begin(&script, &store, &mut state, recipes).unwrap();
    let journaled = store.load().unwrap().unwrap().image_replacement.unwrap().uncommitted;
    assert_eq!(journaled.as_deref(), Some(hash.as_str()), "saved before the build");
    drive(&script, &store, &mut state, &|_| {}).unwrap();
    drop(store);
    let built = crate::cloud_runtime::state::UncommittedImage {
        revision: "c".repeat(40),
        hash,
    };
    assert_eq!(fixture.state().uncommitted_image, Some(built));

    // A later rebuild from the commit alone names no uncommitted changes.
    fixture.edit(|state| {
        state.stage = Stage::Ready;
        state.session_restart = None;
    });
    script.uncommitted = Uncommitted::default();
    script.built = digest('d');
    assert_eq!(rebuild_with(&fixture, &script).unwrap(), Driven::Committed);
    assert!(fixture.state().uncommitted_image.is_none());
}

#[test]
fn a_journal_with_a_malformed_uncommitted_hash_is_refused() {
    let fixture = Fixture::new();
    let mut state = fixture.state();
    state
        .begin_replacement(OperationId::generate(), "c".repeat(40))
        .unwrap();
    let before = state.image_replacement.clone();
    for hash in ["", "short", &"G".repeat(64)] {
        assert!(
            state.replacement_takes_uncommitted(Some(hash.into())).is_err(),
            "{hash:?}"
        );
        assert_eq!(state.image_replacement, before);
    }
    state.replacement_takes_uncommitted(Some("e".repeat(64))).unwrap();
}
