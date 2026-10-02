use super::*;

#[test]
fn staging_failures_preserve_all_original_history() {
    for failed_move in [1, 2] {
        let (temp, projects, transcript, artifacts) = staged_fixture();
        let moves = std::cell::Cell::new(0);
        let result = stage_claude_deletion(
            &projects,
            &transcript,
            Some(&artifacts),
            |from, to| {
                moves.set(moves.get() + 1);
                if moves.get() == failed_move {
                    Err(std::io::Error::other("injected staging failure"))
                } else {
                    std::fs::rename(from, to)
                }
            },
            |_| panic!("staging failure must not purge history"),
        );
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(&transcript).expect("transcript"),
            "original transcript"
        );
        assert_eq!(
            std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("child"),
            "original child"
        );
        assert_eq!(std::fs::read_dir(temp.path()).expect("store").count(), 1);
    }
}

#[test]
fn rollback_failure_retains_recoverable_history_outside_discovery() {
    let (temp, projects, transcript, artifacts) = staged_fixture();
    let moves = std::cell::Cell::new(0);
    let outcome = stage_claude_deletion(
        &projects,
        &transcript,
        Some(&artifacts),
        |from, to| {
            moves.set(moves.get() + 1);
            if moves.get() > 1 {
                Err(std::io::Error::other("injected move failure"))
            } else {
                std::fs::rename(from, to)
            }
        },
        |_| panic!("rollback failure must not purge history"),
    )
    .expect("recovery outcome");
    let bundle = std::fs::read_dir(temp.path())
        .expect("store")
        .map(|entry| entry.expect("entry").path())
        .find(|path| path != &projects)
        .expect("recovery bundle");
    let DeletionOutcome::RecoveryRequired { directory, message } = outcome else {
        panic!("must require recovery")
    };
    assert_eq!(directory, bundle);
    assert!(message.contains("restoring the transcript failed"));
    assert_eq!(
        std::fs::read_to_string(bundle.join("transcript.deleted")).expect("transcript"),
        "original transcript"
    );
    assert_eq!(
        std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("child"),
        "original child"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("original-paths.json")).expect("manifest")).expect("paths");
    assert_eq!(manifest["transcript"], transcript.to_string_lossy().as_ref());
    assert!(!bundle.starts_with(&projects));
}

#[test]
fn retained_manifest_prevents_legacy_artifacts_resurrecting_parent_on_reload() {
    let temp = tempfile::tempdir().expect("store");
    let projects = temp.path().join("projects");
    let session = binding(8);
    let transcript = projects.join("project").join(format!("{}.jsonl", session.session_id));
    let artifacts = transcript.with_extension("");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let payload = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user","message":{"content":"retained history"}}).to_string();
    std::fs::write(&transcript, &payload).expect("transcript");
    std::fs::write(artifacts.join("legacy-agent.jsonl"), &payload).expect("legacy artifact");
    let unaffected = binding(9);
    let other = transcript
        .parent()
        .expect("project")
        .join(format!("{}.jsonl", unaffected.session_id));
    std::fs::write(other, serde_json::json!({"sessionId":unaffected.session_id,"cwd":"/example","type":"user","message":{"content":"keep"}}).to_string()).expect("other");
    let moves = std::cell::Cell::new(0);
    let outcome = stage_claude_deletion(
        &projects,
        &transcript,
        Some(&artifacts),
        |from, to| {
            moves.set(moves.get() + 1);
            if moves.get() == 1 {
                std::fs::rename(from, to)
            } else {
                Err(std::io::Error::other("move blocked"))
            }
        },
        |_| panic!("never purge recovery history"),
    )
    .expect("recovery");
    let DeletionOutcome::RecoveryRequired { directory, .. } = outcome else {
        panic!("recovery required")
    };
    for _ in 0..2 {
        let loaded = load_claude_sessions_from_dir(&projects).expect("fresh discovery");
        assert!(!loaded.iter().any(|record| record.session_id == session.session_id));
        assert!(loaded.iter().any(|record| record.session_id == unaffected.session_id));
    }
    std::fs::rename(directory.join("transcript.deleted"), &transcript).expect("manual restore");
    assert!(
        load_claude_sessions_from_dir(&projects)
            .expect("restored discovery")
            .iter()
            .any(|record| record.session_id == session.session_id)
    );
}

#[test]
fn partial_purge_is_cleanup_pending_and_never_a_resumable_failure() {
    let (temp, projects, transcript, artifacts) = staged_fixture();
    let session = binding(6);
    let mut catalog = AgentSessionCatalog {
        sessions: vec![AgentSessionRecord {
            kind: session.kind,
            session_id: session.session_id.clone(),
            cwd: session.cwd.clone(),
            label: None,
            updated_at: 0,
            interactive: true,
        }],
    };
    let report = catalog.delete_with(std::slice::from_ref(&session), &HashSet::new(), |_| {
        stage_claude_deletion(
            &projects,
            &transcript,
            Some(&artifacts),
            |from, to| std::fs::rename(from, to),
            |directory| {
                std::fs::remove_file(directory.join("artifacts/agent.jsonl"))?;
                Err(std::io::Error::other("injected partial purge"))
            },
        )
    });
    assert_eq!(report.deleted.len(), 1);
    assert!(report.failures.is_empty());
    assert_eq!(report.cleanup_warnings.len(), 1);
    let warning = &report.cleanup_warnings[0];
    assert_eq!(warning.session_id, session.session_id);
    assert!(warning.message.contains("injected partial purge"));
    assert!(warning.directory.starts_with(temp.path()));
    assert!(!warning.directory.starts_with(&projects));
    assert!(!transcript.exists());
    assert!(!artifacts.exists());
    assert!(warning.directory.join("transcript.deleted").exists());
    catalog.remove_deleted_sessions(&report);
    assert!(catalog.recent_for(session.kind, session.cwd.as_deref()).is_empty());
}

#[test]
fn successful_staged_purge_removes_every_selected_byte() {
    let (temp, projects, transcript, artifacts) = staged_fixture();
    assert!(matches!(
        stage_claude_deletion(
            &projects,
            &transcript,
            Some(&artifacts),
            |from, to| std::fs::rename(from, to),
            |path| std::fs::remove_dir_all(path)
        )
        .expect("delete"),
        DeletionOutcome::Removed
    ));
    assert!(!transcript.exists());
    assert!(!artifacts.exists());
    assert_eq!(std::fs::read_dir(temp.path()).expect("store").count(), 1);
}
