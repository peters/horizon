use super::*;

struct Fixture {
    directory: tempfile::TempDir,
    store: WorkStore,
    input: HookInput,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("transcript.jsonl");
        fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"private prompt\"}}\n",
        )
        .expect("transcript");
        let store = WorkStore::new(directory.path());
        let input: HookInput = serde_json::from_value(serde_json::json!({
            "session_id": "session", "prompt_id": "prompt", "cwd": directory.path(),
            "transcript_path": path, "hook_event_name": "SessionStart", "source": "startup",
            "prompt": "private prompt", "tool_input": {"secret": "private tool argument"}
        }))
        .expect("hook");
        store
            .register_owner("panel", PanelKind::Claude, "host", Some("session"), directory.path())
            .expect("register launch");
        let mut fixture = Self {
            directory,
            store,
            input,
        };
        fixture.event("SessionStart", 1);
        fixture.event("UserPromptSubmit", 2);
        fixture
    }

    fn event(&mut self, name: &str, now: i64) {
        self.input.event.hook_event_name = name.into();
        self.store
            .apply_hook("panel", PanelKind::Claude, "host", &self.input, now)
            .expect("hook");
    }

    fn handoff(&self) -> SuspendRecord {
        SuspendRecord {
            kind: PanelKind::Claude,
            before_cancel: Some(
                TranscriptSnapshot::read(&self.input.transcript_path, PanelKind::Claude).expect("snapshot"),
            ),
            panel_local_id: "panel".into(),
            session_id: "session".into(),
            prompt_id: "prompt".into(),
            generation: self
                .store
                .read("panel")
                .expect("read")
                .expect("record")
                .ledger
                .generation,
            suspended_at_millis: 3,
            cwd: self.directory.path().display().to_string(),
            repo_fingerprint: Some("repo".into()),
            final_transcript: None,
            cancelled_by_horizon: true,
        }
    }

    fn seal(&mut self) {
        self.store.save_handoff("host", self.handoff()).expect("handoff");
        self.input.event.reason = Some("other".into());
        self.event("SessionEnd", 4);
        let snapshot = TranscriptSnapshot::read(&self.input.transcript_path, PanelKind::Claude).expect("snapshot");
        self.store.finish_handoff("panel", "host", snapshot).expect("seal");
    }
}

#[test]
fn incomplete_pre_cancel_evidence_cannot_be_saved() {
    let fixture = Fixture::new();
    let valid = fixture.handoff();
    for state in [
        None,
        Some(TurnState::Unknown),
        Some(TurnState::Blocked),
        Some(TurnState::Finished),
        Some(TurnState::Failed),
        Some(TurnState::Interrupted),
    ] {
        let mut incomplete = valid.clone();
        incomplete.before_cancel = state.map(|state| TranscriptSnapshot {
            state,
            ..valid.before_cancel.clone().expect("working snapshot")
        });
        assert!(fixture.store.save_handoff("host", incomplete).is_err());
        assert!(
            fixture
                .store
                .read("panel")
                .expect("read")
                .expect("record")
                .handoff
                .is_none()
        );
    }
    fixture.store.save_handoff("host", valid).expect("complete evidence");
    assert!(
        fixture
            .store
            .read("panel")
            .expect("read")
            .expect("record")
            .handoff
            .is_some()
    );
}

#[test]
fn persistent_handoff_round_trips_and_is_claimed_once() {
    let mut fixture = Fixture::new();
    fixture.seal();
    let reopened = WorkStore::new(fixture.directory.path());
    let record = reopened.read("panel").expect("read").expect("record");
    assert_eq!(record.ledger.state, TurnState::Working);
    assert_eq!(record.handoff.as_ref().expect("handoff").session_id, "session");
    assert!(reopened.claim_handoff(&record).expect("claim"));
    assert!(!reopened.claim_handoff(&record).expect("second claim"));
    let json = fs::read_to_string(reopened.record_path("panel").expect("path")).expect("json");
    assert!(!json.contains("private prompt"));
    assert!(!json.contains("private tool argument"));
}

#[test]
fn a_transcript_advanced_after_classification_cannot_be_claimed() {
    let mut fixture = Fixture::new();
    fixture.seal();
    let record = fixture.store.read("panel").expect("read").expect("record");
    fs::write(&fixture.input.transcript_path, "{}\n").expect("advance transcript");
    assert!(!fixture.store.claim_handoff(&record).expect("claim"));
}

#[test]
fn stale_hosts_and_rebound_sessions_cannot_modify_current_work() {
    let mut fixture = Fixture::new();
    fixture.input.event.hook_event_name = "Stop".into();
    assert!(
        fixture
            .store
            .apply_hook("panel", PanelKind::Claude, "old-host", &fixture.input, 3)
            .is_err()
    );
    fixture.input.event.session_id = "another-session".into();
    assert!(
        fixture
            .store
            .apply_hook("panel", PanelKind::Claude, "host", &fixture.input, 3)
            .is_err()
    );
    fixture.input.event.hook_event_name = "SessionStart".into();
    assert!(
        fixture
            .store
            .apply_hook("panel", PanelKind::Claude, "host", &fixture.input, 4)
            .is_err()
    );
    fixture
        .store
        .register_owner(
            "panel",
            PanelKind::Claude,
            "new-host",
            Some("another-session"),
            fixture.directory.path(),
        )
        .expect("register new launch");
    fixture
        .store
        .apply_hook("panel", PanelKind::Claude, "new-host", &fixture.input, 5)
        .expect("new start");
    assert!(
        fixture
            .store
            .apply_hook("panel", PanelKind::Claude, "host", &fixture.input, 6)
            .is_err()
    );
    let record = fixture.store.read("panel").expect("read").expect("record");
    assert_eq!(record.ledger.session_id, "another-session");
    assert_eq!(record.ledger.state, TurnState::Unknown);
    assert!(record.handoff.is_none());
}

#[test]
fn a_permission_racing_suspend_prevents_the_handoff() {
    let mut fixture = Fixture::new();
    fixture.event("PermissionRequest", 3);
    assert!(fixture.store.save_handoff("host", fixture.handoff()).is_err());
    assert!(
        fixture
            .store
            .read("panel")
            .expect("read")
            .expect("record")
            .handoff
            .is_none()
    );
}

#[test]
fn stale_sealing_and_claims_cannot_replace_new_turn_evidence() {
    let mut fixture = Fixture::new();
    fixture.seal();
    let old = fixture.store.read("panel").expect("read").expect("record");
    fixture.event("SessionStart", 5);
    assert!(!fixture.store.claim_handoff(&old).expect("claim"));
    let snapshot = TranscriptSnapshot::read(&fixture.input.transcript_path, PanelKind::Claude).expect("snapshot");
    assert!(fixture.store.finish_handoff("panel", "host", snapshot).is_err());
}

#[test]
fn untrusted_record_paths_and_future_schemas_are_rejected() {
    let fixture = Fixture::new();
    for id in ["", "../escape", "a/b", "a\\b", "."] {
        assert!(fixture.store.read(id).is_err());
    }
    let path = fixture.store.record_path("panel").expect("path");
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).expect("bytes")).expect("json");
    record["version"] = 2.into();
    fs::write(&path, serde_json::to_vec(&record).expect("json")).expect("write");
    assert!(fixture.store.read("panel").is_err());
}

#[cfg(unix)]
#[test]
fn work_records_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let mode = fs::metadata(fixture.store.record_path("panel").expect("path"))
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn failed_hooks_poison_only_their_launch_even_while_the_record_is_locked() {
    let mut fixture = Fixture::new();
    fixture
        .store
        .invalidate("panel", "other-host")
        .expect("obsolete invalidation is inert");
    assert!(fixture.store.read("panel").expect("record").is_some());
    let lock = fixture.store.lock("panel").expect("hold record lock");
    fixture.input.event.hook_event_name = "PermissionRequest".into();
    assert!(
        fixture
            .store
            .apply_hook("panel", PanelKind::Claude, "host", &fixture.input, 4)
            .is_err()
    );
    fixture
        .store
        .invalidate("panel", "host")
        .expect("invalidate independently of lock");
    assert!(fixture.store.read("panel").is_err());
    drop(lock);
    fixture.input.event.hook_event_name = "SessionEnd".into();
    assert!(
        fixture
            .store
            .apply_hook("panel", PanelKind::Claude, "host", &fixture.input, 5)
            .is_err()
    );
    fixture
        .store
        .register_owner(
            "panel",
            PanelKind::Claude,
            "fresh-host",
            Some("session"),
            fixture.directory.path(),
        )
        .expect("new launch");
    assert_eq!(
        fixture.store.read("panel").expect("read").expect("record").ledger.state,
        TurnState::Unknown
    );
    fixture
        .store
        .invalidate("panel", "host")
        .expect("late failure from old launch");
    assert!(fixture.store.read("panel").expect("new launch unaffected").is_some());
}

#[test]
fn failure_during_claim_persistence_sacrifices_the_claim_without_authorizing_resume() {
    let mut fixture = Fixture::new();
    fixture.seal();
    let record = fixture.store.read("panel").expect("read").expect("record");
    let result = fixture.store.claim_with(&record, || {
        fixture.store.invalidate("panel", "host").expect("invalidate");
    });
    assert!(result.is_err());
    assert!(
        fixture
            .store
            .read_raw("panel")
            .expect("read raw")
            .expect("record")
            .consumed
    );
    assert!(fixture.store.claim_handoff(&record).is_err());
}

#[test]
fn a_new_owner_cannot_claim_an_old_generations_handoff() {
    let mut fixture = Fixture::new();
    fixture.seal();
    fixture
        .store
        .register_owner(
            "panel",
            PanelKind::Claude,
            "next",
            Some("session"),
            fixture.directory.path(),
        )
        .expect("new owner");
    let current = fixture.store.read("panel").expect("read").expect("record");
    assert!(current.handoff.is_some());
    assert!(
        !fixture
            .store
            .claim_handoff(&current)
            .expect("cannot claim stale generation")
    );
}

#[test]
fn an_invalidated_launch_does_not_pass_its_handoff_to_a_new_owner() {
    let mut fixture = Fixture::new();
    fixture.seal();
    fixture.store.invalidate("panel", "host").expect("invalidate");
    fixture
        .store
        .register_owner(
            "panel",
            PanelKind::Claude,
            "next",
            Some("session"),
            fixture.directory.path(),
        )
        .expect("new owner");
    assert!(
        fixture
            .store
            .read("panel")
            .expect("read")
            .expect("record")
            .handoff
            .is_none()
    );
}

#[test]
fn inaccessible_health_uses_record_removal_without_deleting_a_new_owner() {
    let fixture = Fixture::new();
    let health = fixture.store.health_path("panel", "host").expect("health");
    fs::remove_file(&health).expect("remove marker");
    fs::create_dir(&health).expect("inaccessible marker");
    fixture
        .store
        .invalidate("panel", "host")
        .expect("fallback invalidation");
    assert!(fixture.store.read("panel").expect("read").is_none());
    fixture
        .store
        .register_owner(
            "panel",
            PanelKind::Claude,
            "next",
            Some("session"),
            fixture.directory.path(),
        )
        .expect("new owner");
    fixture.store.invalidate("panel", "host").expect("old invalidation");
    assert_eq!(
        fixture.store.read("panel").expect("read").expect("record").owner_token,
        "next"
    );
}

#[test]
fn unrelated_tool_events_do_not_rewrite_the_ledger() {
    let mut fixture = Fixture::new();
    let path = fixture.store.record_path("panel").expect("path");
    let before = fs::read(&path).expect("before");
    fixture.input.event.tool_name = Some("Read".into());
    fixture.event("PreToolUse", 3);
    fixture.event("PostToolUse", 4);
    assert_eq!(fs::read(path).expect("after"), before);
}

#[test]
fn registration_prunes_old_markers_without_allowing_old_hooks_to_invalidate_current_owner() {
    let fixture = Fixture::new();
    for index in 0..20 {
        let owner = format!("owner-{index}");
        fixture
            .store
            .register_owner(
                "panel",
                PanelKind::Claude,
                &owner,
                Some("session"),
                fixture.directory.path(),
            )
            .expect("register");
        fixture.store.invalidate("panel", "host").expect("delayed old hook");
        assert_eq!(
            fixture.store.read("panel").expect("read").expect("record").owner_token,
            owner
        );
        assert_eq!(
            fs::read_dir(fixture.store.health_directory("panel").expect("health directory"))
                .expect("markers")
                .count(),
            1
        );
    }
}

#[test]
fn abandoned_hook_vetoes_reads_even_after_an_overlapping_hook_finishes() {
    let mut fixture = Fixture::new();
    let abandoned = fixture.store.begin_hook("panel", "host").expect("first invocation");
    let completed = fixture.store.begin_hook("panel", "host").expect("second invocation");
    fixture.event("Stop", 3);
    fixture
        .store
        .finish_hook("panel", "host", &completed)
        .expect("complete second");
    assert!(fixture.store.read("panel").is_err());
    fixture
        .store
        .finish_hook("panel", "host", &abandoned)
        .expect("complete first");
    assert!(fixture.store.read("panel").expect("healthy").is_some());
}

#[test]
fn pending_hook_at_claim_authorization_vetoes_dispatch() {
    let mut fixture = Fixture::new();
    fixture.seal();
    let expected = fixture.store.read("panel").expect("read").expect("record");
    assert!(
        fixture
            .store
            .claim_with(&expected, || {
                fixture.store.begin_hook("panel", "host").expect("racing hook");
            })
            .is_err()
    );
}

#[test]
fn new_owner_cannot_inherit_an_abandoned_handoff() {
    let mut fixture = Fixture::new();
    fixture.seal();
    let abandoned = fixture.store.begin_hook("panel", "host").expect("old hook");
    fixture
        .store
        .register_owner(
            "panel",
            PanelKind::Claude,
            "new-host",
            Some("session"),
            fixture.directory.path(),
        )
        .expect("new owner");
    let record = fixture.store.read("panel").expect("new health").expect("record");
    assert!(record.handoff.is_none());
    assert!(fixture.store.finish_hook("panel", "host", &abandoned).is_err());
    fixture.store.invalidate("panel", "host").expect("late invalidation");
    assert!(fixture.store.read("panel").expect("new still healthy").is_some());
}

#[test]
fn panel_paths_preserve_identity_on_case_insensitive_filesystems() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let store = WorkStore::new(directory.path());
    let ids = [
        "Panel-A".to_owned(),
        "panel-a".to_owned(),
        "CON".to_owned(),
        "nul".to_owned(),
        "P".repeat(128),
    ];
    let mut paths = std::collections::HashSet::new();
    for panel in &ids {
        store
            .register_owner(panel, PanelKind::Claude, "owner", None, directory.path())
            .expect("register");
        let record_path = store.record_path(panel).expect("record path");
        assert!(paths.insert(record_path.to_string_lossy().to_ascii_lowercase()));
        assert!(record_path.components().all(|part| part.as_os_str().len() <= 255));
        let token = store.begin_hook(panel, "owner").expect("begin hook");
        assert!(store.read(panel).is_err());
        store.finish_hook(panel, "owner", &token).expect("finish hook");
    }
    for panel in &ids {
        assert_eq!(store.read(panel).expect("read").expect("record").panel_local_id, *panel);
    }
    store.invalidate("Panel-A", "owner").expect("invalidate one panel");
    assert!(store.read("Panel-A").is_err());
    assert!(store.read("panel-a").expect("other panel").is_some());
}

fn staging_files(store: &WorkStore) -> Vec<std::ffi::OsString> {
    fs::read_dir(&store.root)
        .expect("store root")
        .map(|entry| entry.expect("entry").file_name())
        .filter(|name| name.to_string_lossy().starts_with(STAGING_PREFIX))
        .collect()
}

#[test]
fn longest_panel_ids_round_trip_beyond_windows_max_path() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let store = WorkStore::new(directory.path());
    let panel = "p".repeat(128);
    let record_path = store.record_path(&panel).expect("record path");
    assert!(record_path.as_os_str().len() > 260, "{}", record_path.display());
    for owner in ["first", "second"] {
        store
            .register_owner(&panel, PanelKind::Codex, owner, Some("session"), directory.path())
            .expect("register");
        let record = store.read(&panel).expect("read").expect("record");
        assert_eq!(
            (record.panel_local_id.as_str(), record.owner_token.as_str()),
            (panel.as_str(), owner)
        );
    }
    assert!(staging_files(&store).is_empty());
    store.invalidate(&panel, "second").expect("invalidate");
    assert!(store.read(&panel).is_err());
}

#[test]
fn failed_publication_leaves_no_staging_file() {
    let fixture = Fixture::new();
    let mut record = fixture.store.read("panel").expect("read").expect("record");
    record.panel_local_id = "blocked".into();
    let blocked = fixture.store.record_path("blocked").expect("blocked path");
    fs::create_dir_all(blocked.join("occupied")).expect("directory in the record's place");
    assert!(fixture.store.write(&record).is_err());
    assert!(staging_files(&fixture.store).is_empty());
}

#[cfg(windows)]
#[test]
fn published_records_are_not_marked_temporary() {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
    let fixture = Fixture::new();
    let attributes = fs::metadata(fixture.store.record_path("panel").expect("path"))
        .expect("metadata")
        .file_attributes();
    assert_eq!(attributes & FILE_ATTRIBUTE_TEMPORARY, 0);
}
