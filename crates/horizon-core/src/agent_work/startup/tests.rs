use super::*;
#[cfg(target_os = "linux")]
use std::io::Write as _;

#[test]
fn restore_budget_is_bounded_and_resets_on_scope_exit() {
    assert_eq!(REMAINING.get(), 0);
    {
        let _budget = RestoreBudget::new(2);
        for expected in [1, 0] {
            let mut plan = StartupPlan {
                owner: None,
                state: WorkContinuation::default(),
                seed: Some("brief".into()),
            };
            let mut args = vec!["-ic".into(), "agent --resume session".into()];
            plan.seed_if_attached(true, &mut args);
            assert!(args[1].ends_with("'brief'"));
            assert_eq!(REMAINING.get(), expected);
        }
        {
            let _nested = RestoreBudget::new(4);
            assert_eq!(REMAINING.get(), 4);
        }
        assert_eq!(REMAINING.get(), 0);
    }
    assert_eq!(REMAINING.get(), 0);
}

#[test]
fn unavailable_hooks_do_not_send_seed_or_consume_capacity() {
    let _budget = RestoreBudget::new(1);
    let mut plan = StartupPlan {
        owner: None,
        state: WorkContinuation::default(),
        seed: Some("brief".into()),
    };
    let mut args = vec!["-ic".into(), "agent --resume session".into()];
    plan.seed_if_attached(false, &mut args);
    assert_eq!(args[1], "agent --resume session");
    assert_eq!(plan.state.pending(), Some(AskReason::MissingEvidence));
    assert_eq!(REMAINING.get(), 1);
}

#[test]
fn user_input_dismisses_stale_resume_offer() {
    let state = WorkContinuation::ask(AskReason::Downtime);
    state.note_input(&[]);
    assert_eq!(state.pending(), Some(AskReason::Downtime));
    state.note_input(b"new request\r");
    assert_eq!(state.pending(), None);
}

#[test]
fn seeded_prompt_is_a_single_quoted_argument() {
    let mut args = vec!["-ic".into(), "agent --resume session".into()];
    assert!(append_seed(
        PanelKind::Claude,
        &mut args,
        "don't execute $(false); `false`"
    ));
    assert_eq!(args[1], "agent --resume session 'don'\\''t execute $(false); `false`'");
    let mut direct = vec!["--resume".into(), "session".into()];
    assert!(!append_seed(PanelKind::Claude, &mut direct, "brief"));
}

#[cfg(target_os = "linux")]
struct Fixture {
    home: tempfile::TempDir,
    store: WorkStore,
    policy: super::super::ResumePolicy,
    input: super::super::HookInput,
}
#[cfg(target_os = "linux")]
impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home");
        std::fs::create_dir_all(home.path().join(".claude/sessions")).expect("registry");
        let repo = git2::Repository::init(home.path()).expect("repo");
        let mut index = repo.index().expect("index");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("tree");
        let tree = repo.find_tree(tree_id).expect("tree");
        let sig = git2::Signature::now("Fixture", "fixture@example.invalid").expect("signature");
        repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
            .expect("commit");
        // Keep all test metadata outside the repository fingerprint.
        std::fs::write(home.path().join(".git/info/exclude"), "*\n").expect("ignore fixture metadata");
        let store = WorkStore::new(&home.path().join(".horizon"));
        let path = home.path().join("transcript.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"message\":{\"content\":\"work\"}}\n").expect("transcript");
        store
            .register_owner("panel", PanelKind::Claude, "owner", Some("session"), home.path())
            .expect("owner");
        let input = serde_json::from_value(serde_json::json!({"session_id":"session", "prompt_id":"prompt", "hook_event_name":"UserPromptSubmit", "cwd":home.path(),"transcript_path":path})).expect("hook");
        store
            .apply_hook("panel", PanelKind::Claude, "owner", &input, current_unix_millis() - 100)
            .expect("working");
        Self {
            home,
            store,
            input,
            policy: super::super::ResumePolicy {
                enabled: true,
                ..Default::default()
            },
        }
    }
    fn seal(&mut self) {
        let record = self.store.read("panel").expect("read").expect("record");
        let before = TranscriptSnapshot::read(&self.input.transcript_path, PanelKind::Claude).expect("before");
        self.store
            .save_handoff(
                "owner",
                super::super::SuspendRecord {
                    kind: PanelKind::Claude,
                    before_cancel: Some(before),
                    panel_local_id: "panel".into(),
                    session_id: "session".into(),
                    prompt_id: "prompt".into(),
                    generation: record.ledger.generation,
                    suspended_at_millis: current_unix_millis() - 50,
                    cwd: self
                        .home
                        .path()
                        .canonicalize()
                        .expect("cwd")
                        .to_str()
                        .expect("utf8")
                        .into(),
                    repo_fingerprint: super::super::repository::fingerprint(self.home.path()),
                    final_transcript: None,
                    cancelled_by_horizon: true,
                },
            )
            .expect("handoff");
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&self.input.transcript_path)
                .expect("tail"),
            "{{\"type\":\"user\",\"message\":{{\"content\":\"[Request interrupted by user]\"}}}}"
        )
        .expect("cancel");
        self.input.event.hook_event_name = "SessionEnd".into();
        self.input.event.reason = Some("other".into());
        self.store
            .apply_hook("panel", PanelKind::Claude, "owner", &self.input, current_unix_millis())
            .expect("ended");
        self.store
            .finish_handoff(
                "panel",
                "owner",
                TranscriptSnapshot::read(&self.input.transcript_path, PanelKind::Claude).expect("final"),
            )
            .expect("seal");
    }
    fn inspect(&self) -> (RestartDecision, Option<String>) {
        inspect_at(
            &WorkLaunch {
                panel: "panel",
                kind: PanelKind::Claude,
                policy: &self.policy,
                cwd: Some(self.home.path()),
                session_id: Some("session"),
                default_command: true,
            },
            "session",
            self.home.path(),
            &self.store,
        )
        .expect("inspect")
    }
}

#[test]
#[cfg(target_os = "linux")]
fn a_clean_handoff_is_claimed_once_and_never_replayed() {
    let _budget = RestoreBudget::new(1);
    let mut fixture = Fixture::new();
    fixture.seal();
    let (decision, seed) = fixture.inspect();
    assert_eq!(decision, RestartDecision::Resume);
    assert!(seed.expect("brief").contains("may have partially completed"));
    assert!(fixture.store.read("panel").expect("read").expect("record").consumed);
    assert_eq!(fixture.inspect().0, RestartDecision::NotResumable);
}

#[cfg(target_os = "linux")]
fn record_later_question(fixture: &mut Fixture) {
    fixture.input.event.prompt_id = Some("later-prompt".into());
    fixture.input.event.source = Some("resume".into());
    fixture.input.event.tool_name = Some("AskUserQuestion".into());
    fixture.input.event.tool_use_id = Some("question".into());
    for name in ["SessionStart", "UserPromptSubmit", "PreToolUse"] {
        fixture.input.event.hook_event_name = name.into();
        fixture
            .store
            .apply_hook(
                "panel",
                PanelKind::Claude,
                "owner",
                &fixture.input,
                current_unix_millis(),
            )
            .expect("later turn");
    }
    std::fs::write(&fixture.input.transcript_path, "{\"type\":\"assistant\",\"message\":{\"stop_reason\":\"tool_use\",\"content\":[{\"type\":\"tool_use\",\"name\":\"AskUserQuestion\"}]}}\n").expect("current question");
}

#[test]
#[cfg(target_os = "linux")]
fn a_later_question_is_manual_even_when_an_old_handoff_remains() {
    let _budget = RestoreBudget::new(5);
    for consumed in [false, true] {
        let mut fixture = Fixture::new();
        fixture.seal();
        if consumed {
            assert_eq!(fixture.inspect().0, RestartDecision::Resume);
            assert_eq!(fixture.inspect().0, RestartDecision::NotResumable);
        }
        record_later_question(&mut fixture);
        for _ in 0..2 {
            assert_eq!(
                fixture.inspect(),
                (RestartDecision::Ask(AskReason::WaitingForUser), None)
            );
            assert_eq!(
                fixture.store.read("panel").expect("read").expect("record").consumed,
                consumed
            );
        }
        fixture.input.event.hook_event_name = "SessionStart".into();
        fixture
            .store
            .apply_hook(
                "panel",
                PanelKind::Claude,
                "owner",
                &fixture.input,
                current_unix_millis(),
            )
            .expect("reopen");
        assert_eq!(
            fixture.inspect(),
            (RestartDecision::Ask(AskReason::WaitingForUser), None)
        );
    }
}

#[cfg(target_os = "linux")]
fn record_later_tool_permission(fixture: &mut Fixture) {
    fixture.input.event.prompt_id = Some("later-tool-prompt".into());
    fixture.input.event.tool_name = Some("Bash".into());
    fixture.input.event.tool_use_id = Some("unapproved-tool".into());
    for event in ["UserPromptSubmit", "PermissionRequest"] {
        fixture.input.event.hook_event_name = event.into();
        fixture
            .store
            .apply_hook(
                "panel",
                PanelKind::Claude,
                "owner",
                &fixture.input,
                current_unix_millis(),
            )
            .expect("later permission");
    }
    writeln!(
        std::fs::OpenOptions::new().append(true).open(&fixture.input.transcript_path).expect("transcript"),
        r#"{{"type":"assistant","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Bash"}}]}}}}"#
    ).expect("pending tool");
}

#[test]
#[cfg(target_os = "linux")]
fn later_tool_permission_remains_manual_across_repeated_restores() {
    let _budget = RestoreBudget::new(5);
    for consumed in [false, true] {
        let mut fixture = Fixture::new();
        fixture.seal();
        if consumed {
            assert_eq!(fixture.inspect().0, RestartDecision::Resume);
            assert_eq!(fixture.inspect(), (RestartDecision::NotResumable, None));
        }
        let previous = fixture.store.read("panel").expect("read").expect("record");
        record_later_tool_permission(&mut fixture);
        assert_eq!(
            fixture.inspect(),
            (RestartDecision::Ask(AskReason::WaitingForUser), None)
        );
        for _ in 0..3 {
            fixture.input.event.hook_event_name = "SessionStart".into();
            fixture.input.event.source = Some("resume".into());
            fixture
                .store
                .apply_hook(
                    "panel",
                    PanelKind::Claude,
                    "owner",
                    &fixture.input,
                    current_unix_millis(),
                )
                .expect("restore");
            assert_eq!(
                fixture.inspect(),
                (RestartDecision::Ask(AskReason::MissingEvidence), None)
            );
            let record = fixture.store.read("panel").expect("read").expect("record");
            assert_eq!(record.consumed, consumed);
            assert_eq!(record.handoff, previous.handoff);
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn changed_consumed_work_does_not_override_terminal_evidence() {
    let _budget = RestoreBudget::new(5);
    let mut fixture = Fixture::new();
    fixture.seal();
    assert_eq!(fixture.inspect().0, RestartDecision::Resume);
    for veto in ["Stop", "StopFailure", "SessionEnd", "Interrupt"] {
        record_later_tool_permission(&mut fixture);
        // Remove the pending permission before the explicit Interrupt so that
        // this covers the ledger's interrupted-state veto independently.
        fixture.input.event.hook_event_name = "UserPromptSubmit".into();
        fixture
            .store
            .apply_hook(
                "panel",
                PanelKind::Claude,
                "owner",
                &fixture.input,
                current_unix_millis(),
            )
            .expect("fresh turn");
        fixture.input.event.hook_event_name = veto.into();
        fixture.input.event.reason = Some("prompt_input_exit".into());
        fixture
            .store
            .apply_hook(
                "panel",
                PanelKind::Claude,
                "owner",
                &fixture.input,
                current_unix_millis(),
            )
            .expect("veto");
        assert_eq!(fixture.inspect(), (RestartDecision::NotResumable, None));
    }
    for terminal in [
        r#"{"type":"user","message":{"content":"[Request interrupted by user]"}}"#,
        r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[]}}"#,
    ] {
        record_later_tool_permission(&mut fixture);
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&fixture.input.transcript_path)
                .expect("transcript"),
            "{terminal}"
        )
        .expect("terminal message");
        assert_eq!(fixture.inspect(), (RestartDecision::NotResumable, None));
    }
}

#[test]
#[cfg(target_os = "linux")]
fn later_questions_do_not_override_completion_exit_or_transcript_interrupts() {
    let _budget = RestoreBudget::new(5);
    let mut fixture = Fixture::new();
    fixture.seal();
    assert_eq!(fixture.inspect().0, RestartDecision::Resume);
    for veto in ["Stop", "StopFailure", "SessionEnd"] {
        record_later_question(&mut fixture);
        fixture.input.event.hook_event_name = veto.into();
        fixture.input.event.reason = Some("prompt_input_exit".into());
        fixture
            .store
            .apply_hook(
                "panel",
                PanelKind::Claude,
                "owner",
                &fixture.input,
                current_unix_millis(),
            )
            .expect("veto");
        assert_eq!(fixture.inspect(), (RestartDecision::NotResumable, None));
    }
    record_later_question(&mut fixture);
    std::fs::write(
        &fixture.input.transcript_path,
        "{\"type\":\"user\",\"message\":{\"content\":\"[Request interrupted by user]\"}}\n",
    )
    .expect("user interrupt");
    assert_eq!(fixture.inspect(), (RestartDecision::NotResumable, None));
}

#[test]
#[cfg(target_os = "linux")]
fn budget_and_uncertain_process_identity_do_not_claim_the_handoff() {
    let _budget = RestoreBudget::new(0);
    let mut fixture = Fixture::new();
    fixture.seal();
    assert_eq!(fixture.inspect().0, RestartDecision::Ask(AskReason::BatchLimit));
    std::fs::write(
        fixture.home.path().join(".claude/sessions/live.json"),
        format!(r#"{{"sessionId":"session","pid":{}}}"#, std::process::id()),
    )
    .expect("live");
    REMAINING.set(5);
    assert_eq!(fixture.inspect().0, RestartDecision::Ask(AskReason::MissingEvidence));
    assert!(!fixture.store.read("panel").expect("read").expect("record").consumed);
}

#[test]
#[cfg(target_os = "linux")]
fn crash_and_permission_evidence_ask_without_a_handoff() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.home.path().join(".claude/sessions/dead.json"),
        r#"{"sessionId":"session","pid":4294967295}"#,
    )
    .expect("stale");
    assert_eq!(fixture.inspect().0, RestartDecision::Ask(AskReason::UncleanShutdown));
    let mut input = fixture.input;
    input.event.hook_event_name = "PermissionRequest".into();
    fixture
        .store
        .apply_hook("panel", PanelKind::Claude, "owner", &input, current_unix_millis())
        .expect("permission");
    let fixture = Fixture { input, ..fixture };
    assert_eq!(fixture.inspect().0, RestartDecision::Ask(AskReason::WaitingForUser));
}

#[test]
#[cfg(target_os = "linux")]
fn uncertain_registry_asks_for_unfinished_work_without_overriding_vetoes() {
    let _budget = RestoreBudget::new(5);
    for registry in ["missing", "malformed", "reused"] {
        for veto in [
            None,
            Some("Stop"),
            Some("StopFailure"),
            Some("Interrupt"),
            Some("SessionEnd"),
        ] {
            let mut fixture = Fixture::new();
            let sessions = fixture.home.path().join(".claude/sessions");
            match registry {
                "missing" => std::fs::remove_dir(&sessions).expect("missing registry"),
                "malformed" => std::fs::write(sessions.join("invalid.json"), "{").expect("malformed registry"),
                _ => std::fs::write(
                    sessions.join("reused.json"),
                    format!(r#"{{"sessionId":"session","pid":{}}}"#, std::process::id()),
                )
                .expect("unrelated live process"),
            }
            if let Some(veto) = veto {
                fixture.input.event.hook_event_name = veto.into();
                fixture.input.event.reason = Some("prompt_input_exit".into());
                fixture
                    .store
                    .apply_hook(
                        "panel",
                        PanelKind::Claude,
                        "owner",
                        &fixture.input,
                        current_unix_millis(),
                    )
                    .expect("terminal event");
            }
            let expected = if veto.is_some() {
                RestartDecision::NotResumable
            } else {
                RestartDecision::Ask(AskReason::MissingEvidence)
            };
            assert_eq!(fixture.inspect(), (expected, None), "{registry}, {veto:?}");
            let record = fixture.store.read("panel").expect("read").expect("record");
            assert!(!record.consumed);
            assert!(record.handoff.is_none());
            for terminal in [
                r#"{"type":"user","message":{"content":"[Request interrupted by user]"}}"#,
                r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[]}}"#,
            ] {
                std::fs::write(&fixture.input.transcript_path, format!("{terminal}\n")).expect("terminal transcript");
                assert_eq!(fixture.inspect(), (RestartDecision::NotResumable, None));
            }
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn missing_registry_and_repository_changes_downgrade_clean_work() {
    let _budget = RestoreBudget::new(1);
    let mut fixture = Fixture::new();
    fixture.seal();
    std::fs::remove_dir(fixture.home.path().join(".claude/sessions")).expect("remove registry");
    assert_eq!(fixture.inspect().0, RestartDecision::Ask(AskReason::MissingEvidence));
    std::fs::create_dir(fixture.home.path().join(".claude/sessions")).expect("registry");
    std::fs::write(fixture.home.path().join(".git/info/exclude"), "").expect("change worktree visibility");
    assert_eq!(fixture.inspect().0, RestartDecision::Ask(AskReason::RepositoryChanged));
}

#[test]
fn invalid_resume_limits_never_enable_unattended_work() {
    for value in ["33", "999999999999999999999999999", "-1", "invalid", ""] {
        assert_eq!(super::parse_resume_limit(value), 0);
    }
    assert_eq!(super::parse_resume_limit("0"), 0);
    assert_eq!(super::parse_resume_limit("3"), 3);
    assert_eq!(super::parse_resume_limit("32"), 32);
}

#[test]
fn missing_identity_keeps_an_opted_in_panel_visible_for_review() {
    let policy = super::super::ResumePolicy {
        enabled: true,
        ..Default::default()
    };
    let plan = StartupPlan::prepare(
        &WorkLaunch {
            panel: "missing-identity",
            kind: PanelKind::Claude,
            policy: &policy,
            cwd: None,
            session_id: None,
            default_command: true,
        },
        true,
        true,
    );
    assert_eq!(plan.state.pending(), Some(AskReason::MissingEvidence));
    assert!(plan.seed.is_none());
}
