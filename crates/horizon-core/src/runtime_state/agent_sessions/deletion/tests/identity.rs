use super::*;

#[test]
fn invalid_live_registry_aborts_before_touching_transcript_or_artifacts() {
    let home = tempfile::tempdir().expect("home");
    let registry = home.path().join(".claude/sessions");
    let project = home.path().join(".claude/projects/example");
    std::fs::create_dir_all(&registry).expect("registry");
    std::fs::create_dir_all(&project).expect("project");
    let session = binding(801);
    let transcript = project.join(format!("{}.jsonl", session.session_id));
    let artifacts = project.join(&session.session_id);
    std::fs::create_dir_all(artifacts.join("subagents")).expect("artifacts");
    std::fs::write(&transcript, "original conversation").expect("transcript");
    std::fs::write(artifacts.join("subagents/agent.jsonl"), "original subagent").expect("subagent");
    std::fs::write(registry.join("101.json"), "malformed").expect("bad registry");
    assert!(delete_claude_session(home.path(), &session).is_err());
    assert_eq!(
        std::fs::read_to_string(&transcript).expect("preserved transcript"),
        "original conversation"
    );
    assert_eq!(
        std::fs::read_to_string(artifacts.join("subagents/agent.jsonl")).expect("preserved subagent"),
        "original subagent"
    );
}

#[test]
fn transcript_deletion_removes_only_selected_conversation_and_its_subagents() {
    let temp = tempfile::tempdir().expect("temporary store");
    let project = temp.path().join("project");
    std::fs::create_dir(&project).expect("project");
    let session = binding(1);
    let path = project.join(format!("{}.jsonl", session.session_id));
    let contents = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user","message":{"content":"synthetic conversation"}}).to_string();
    std::fs::write(&path, contents).expect("transcript");
    let children = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&children).expect("subagents");
    std::fs::write(children.join("agent.jsonl"), "synthetic child").expect("child");
    let unrelated = project.join("unrelated.jsonl");
    std::fs::write(&unrelated, "keep").expect("unrelated");
    delete_claude_transcript(temp.path(), &session).expect("delete");
    assert!(!path.exists());
    assert!(!children.exists());
    assert!(unrelated.exists());
}

#[test]
fn ambiguous_transcript_identities_preserve_all_saved_history() {
    let session = binding(802);
    let valid = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user"}).to_string();
    let conflicting =
        serde_json::json!({"sessionId":binding(803).session_id,"cwd":"/example","type":"user"}).to_string();
    let filler = serde_json::json!({"type":"progress","content":"x".repeat(2048)}).to_string() + "\n";
    for contents in [
        serde_json::json!({"cwd":"/example","type":"user"}).to_string(),
        format!("{conflicting}\n{valid}\n"),
        format!(
            "{valid}\n{}{conflicting}\n{}{valid}\n",
            filler.repeat(50),
            filler.repeat(50)
        ),
        format!("{valid}\n{{\"sessionId\":\"\"}}\n"),
        format!("{valid}\n{{\"sessionId\":null}}\n"),
        format!("{valid}\n{{\"sessionId\":42}}\n"),
        format!(
            "{{\"sessionId\":\"{}\",\"sessionId\":\"{}\",\"cwd\":\"/example\"}}\n",
            binding(803).session_id,
            session.session_id
        ),
        format!(
            "{{\"sessionId\":\"\",\"sessionId\":\"{}\",\"cwd\":\"/example\"}}\n",
            session.session_id
        ),
        format!(
            "{{\"sessionId\":42,\"sessionId\":\"{}\",\"cwd\":\"/example\"}}\n",
            session.session_id
        ),
        format!("{valid}\n[\"{}\"]\n", session.session_id),
        format!("{valid}\nmalformed\n"),
    ] {
        let temp = tempfile::tempdir().expect("private store");
        let project = temp.path().join("projects/example");
        let artifacts = project.join(&session.session_id).join("subagents");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let path = project.join(format!("{}.jsonl", session.session_id));
        std::fs::write(&path, &contents).expect("transcript");
        std::fs::write(artifacts.join("agent.jsonl"), "saved child history").expect("subagent");
        let discovered = load_claude_project_session_summary(&path, 0)
            .expect("discovery")
            .expect("record");
        assert_eq!(discovered.session_id, session.session_id);
        assert!(delete_claude_transcript(&temp.path().join("projects"), &session).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("retained transcript"), contents);
        assert_eq!(
            std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("retained child"),
            "saved child history"
        );
        assert_eq!(std::fs::read_dir(temp.path()).expect("store").count(), 1);
    }
}

#[test]
fn conflicting_or_invalid_folder_metadata_preserves_complete_history() {
    let session = binding(805);
    let valid = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user"}).to_string();
    let filler = serde_json::json!({"type":"progress","content":"x".repeat(2048)}).to_string() + "\n";
    for folder_record in [
        r#"{"cwd":"/different"}"#,
        r#"{"cwd":""}"#,
        r#"{"cwd":null}"#,
        r#"{"cwd":42}"#,
        r#"{"cwd":"/different","cwd":"/example"}"#,
    ] {
        let temp = tempfile::tempdir().expect("private store");
        let projects = temp.path().join("projects");
        let project = projects.join("example");
        let artifacts = project.join(&session.session_id).join("subagents");
        std::fs::create_dir_all(&artifacts).expect("artifacts");
        let path = project.join(format!("{}.jsonl", session.session_id));
        let contents = format!(
            "{valid}\n{}{folder_record}\n{}{valid}\n",
            filler.repeat(50),
            filler.repeat(50)
        );
        std::fs::write(&path, &contents).expect("transcript");
        std::fs::write(artifacts.join("agent.jsonl"), "saved child history").expect("subagent");
        let discovered = load_claude_project_session_summary(&path, 0)
            .expect("discovery")
            .expect("record");
        assert_eq!(discovered.cwd, normalize_cwd(session.cwd.as_deref()));
        assert!(delete_claude_transcript(&projects, &session).is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("retained transcript"), contents);
        assert_eq!(
            std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("retained child"),
            "saved child history"
        );
        assert_eq!(std::fs::read_dir(temp.path()).expect("store").count(), 1);
    }
}

#[test]
fn empty_folder_matching_catalog_scope_preserves_history() {
    let temp = tempfile::tempdir().expect("private store");
    let project = temp.path().join("example");
    let mut session = binding(806);
    session.cwd = Some(String::new());
    let path = project.join(format!("{}.jsonl", session.session_id));
    let artifacts = path.with_extension("").join("subagents");
    std::fs::create_dir_all(&artifacts).expect("artifacts");
    let contents = serde_json::json!({"sessionId":session.session_id,"cwd":"","type":"user"}).to_string();
    std::fs::write(&path, &contents).expect("transcript");
    std::fs::write(artifacts.join("agent.jsonl"), "saved child history").expect("subagent");
    let discovered = load_claude_project_session_summary(&path, 0)
        .expect("discovery")
        .expect("record");
    assert_eq!(discovered.cwd, session.cwd);
    assert!(delete_claude_transcript(temp.path(), &session).is_err());
    assert_eq!(std::fs::read_to_string(path).expect("retained transcript"), contents);
    assert_eq!(
        std::fs::read_to_string(artifacts.join("agent.jsonl")).expect("retained subagent"),
        "saved child history"
    );
}

#[test]
fn repeated_matching_identity_and_metadata_records_allow_deletion() {
    let temp = tempfile::tempdir().expect("private store");
    let project = temp.path().join("example");
    std::fs::create_dir(&project).expect("project");
    let session = binding(804);
    let path = project.join(format!("{}.jsonl", session.session_id));
    let record = serde_json::json!({"sessionId":session.session_id,"cwd":"/example","type":"user"}).to_string();
    std::fs::write(&path, format!("{record}\n{{\"type\":\"progress\"}}\n\n{record}\n")).expect("transcript");
    delete_claude_transcript(temp.path(), &session).expect("unambiguous deletion");
    assert!(!path.exists());
}

#[test]
#[cfg(unix)]
fn linked_transcripts_never_delete_their_target() {
    let temp = tempfile::tempdir().expect("private store");
    let projects = temp.path().join("projects");
    let project = projects.join("project");
    std::fs::create_dir_all(&project).expect("project");
    let session = binding(5);
    let outside = temp.path().join("outside.jsonl");
    std::fs::write(&outside, "keep").expect("outside");
    std::os::unix::fs::symlink(&outside, project.join(format!("{}.jsonl", session.session_id))).expect("link");
    assert!(delete_claude_transcript(&projects, &session).is_err());
    assert_eq!(std::fs::read_to_string(outside).expect("target preserved"), "keep");
}

#[test]
fn oversized_record_preserves_history_and_boundary_record_deletes() {
    for overflow in [false, true] {
        let temp = tempfile::tempdir().expect("private store");
        let session = binding(807);
        let project = temp.path().join("project");
        std::fs::create_dir(&project).expect("project");
        let path = project.join(format!("{}.jsonl", session.session_id));
        let artifacts = path.with_extension("");
        std::fs::create_dir(&artifacts).expect("artifacts");
        std::fs::write(artifacts.join("child.jsonl"), "saved child").expect("child");
        let mut contents = serde_json::json!({"sessionId":session.session_id,"cwd":"/example"}).to_string();
        contents.extend(std::iter::repeat_n(
            ' ',
            MAX_TRANSCRIPT_RECORD_BYTES - contents.len() - 1,
        ));
        contents.push('\n');
        if overflow {
            contents.insert(0, ' ');
        }
        std::fs::write(&path, &contents).expect("transcript");
        let result = delete_claude_transcript(temp.path(), &session);
        if overflow {
            assert!(result.is_err());
            assert_eq!(std::fs::read_to_string(path).expect("preserved transcript"), contents);
            assert_eq!(
                std::fs::read_to_string(artifacts.join("child.jsonl")).expect("preserved child"),
                "saved child"
            );
        } else {
            result.expect("boundary is valid");
            assert!(!path.exists());
            assert!(!artifacts.exists());
        }
    }
}
