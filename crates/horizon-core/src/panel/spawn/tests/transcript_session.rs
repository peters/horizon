use super::*;

#[test]
fn prepare_transcript_restore_treats_empty_root_as_fresh_state() {
    let transcript_root = tempfile::tempdir().expect("tempdir");

    let (_, replay_bytes, had_persisted_state) = prepare_transcript_restore(
        PanelId(1),
        PanelKind::Ssh,
        Some(transcript_root.path().to_path_buf()),
        "ssh-panel",
    );

    assert!(replay_bytes.is_empty());
    assert!(!had_persisted_state);
}

#[test]
fn prepare_transcript_restore_detects_empty_persisted_transcript() {
    let transcript_root = tempfile::tempdir().expect("tempdir");
    std::fs::write(transcript_root.path().join("ssh-panel.bin"), b"").expect("write transcript");

    let (_, replay_bytes, had_persisted_state) = prepare_transcript_restore(
        PanelId(1),
        PanelKind::Ssh,
        Some(transcript_root.path().to_path_buf()),
        "ssh-panel",
    );

    assert!(replay_bytes.is_empty());
    assert!(had_persisted_state);
}

#[test]
fn initial_agent_prompt_skips_transcript_restore_and_capture() {
    let transcript_root = tempfile::tempdir().expect("tempdir");
    let session_path = transcript_root.path().join("prompt-panel.session");
    std::fs::write(&session_path, b"PRIVATE persisted transcript bytes").expect("write transcript");

    let (transcript, replay_bytes, had_persisted_state) = prepare_transcript_launch(
        PanelId(1),
        PanelKind::Shell,
        Some(transcript_root.path().to_path_buf()),
        "prompt-panel",
        true,
    );

    assert!(transcript.is_none());
    assert!(replay_bytes.is_empty());
    assert!(!had_persisted_state);
    assert_eq!(
        std::fs::read(session_path).expect("original transcript remains untouched"),
        b"PRIVATE persisted transcript bytes"
    );
}

#[cfg(unix)]
#[test]
fn valid_agent_prompt_spawn_leaves_persisted_transcript_state_untouched() {
    let transcript_root = tempfile::tempdir().expect("tempdir");
    let session_path = transcript_root.path().join("prompt-agent.session");
    let history_path = transcript_root.path().join("prompt-agent.bin");
    std::fs::write(&session_path, b"PRIVATE live transcript bytes").expect("write session transcript");
    std::fs::write(&history_path, b"PRIVATE history bytes").expect("write history transcript");
    let options = PanelOptions {
        kind: PanelKind::Codex,
        command: Some("/bin/true".to_string()),
        local_id: Some("prompt-agent".to_string()),
        transcript_root: Some(transcript_root.path().to_path_buf()),
        initial_agent_prompt: Some("one-shot setup prompt".to_string()),
        agent_login_shell: true,
        ..PanelOptions::default()
    };

    let panel = spawn_panel(PanelId(11), WorkspaceId(1), options).expect("spawn valid agent prompt panel");
    drop(panel);

    assert_eq!(
        std::fs::read(session_path).expect("read untouched session transcript"),
        b"PRIVATE live transcript bytes"
    );
    assert_eq!(
        std::fs::read(history_path).expect("read untouched history transcript"),
        b"PRIVATE history bytes"
    );
}

#[test]
fn claude_fresh_launch_preassigns_session_binding() {
    let (binding, should_resume) = resolve_session_binding(
        PanelKind::Claude,
        &PanelResume::Fresh,
        None,
        Some("/repo"),
        None,
        |_| false,
    );

    let binding = binding.expect("fresh Claude panels are bound to their session at launch");
    assert!(!should_resume);
    assert_eq!(binding.kind, PanelKind::Claude);
    assert_eq!(binding.cwd.as_deref(), Some("/repo"));
    assert!(!binding.session_id.is_empty());
    assert!(binding.updated_at.is_some());
}

#[test]
fn non_claude_fresh_launch_stays_unbound() {
    let (binding, should_resume) =
        resolve_session_binding(PanelKind::Codex, &PanelResume::Fresh, None, Some("/repo"), None, |_| {
            false
        });

    assert!(binding.is_none());
    assert!(!should_resume);
}

#[test]
fn claude_binding_resumes_only_when_transcript_exists() {
    let binding = AgentSessionBinding::new(PanelKind::Claude, "session-1".to_string(), None, None, None);

    let (_, resume_with_missing_transcript) = resolve_session_binding(
        PanelKind::Claude,
        &PanelResume::Fresh,
        Some(binding.clone()),
        None,
        None,
        |_| false,
    );
    let (_, resume_with_existing_transcript) = resolve_session_binding(
        PanelKind::Claude,
        &PanelResume::Fresh,
        Some(binding),
        None,
        None,
        |_| true,
    );

    assert!(!resume_with_missing_transcript);
    assert!(resume_with_existing_transcript);
}

#[test]
fn claude_fresh_launch_command_uses_preassigned_session_id() {
    let binding = AgentSessionBinding::new(
        PanelKind::Claude,
        "11111111-2222-3333-4444-555555555555".to_string(),
        None,
        None,
        None,
    );

    let (_, args) = resolve_launch_command(
        None,
        Vec::new(),
        None,
        PanelKind::Claude,
        AgentLaunchContext {
            resume: &PanelResume::Fresh,
            session_binding: Some(&binding),
            should_resume_binding: false,
            initial_agent_prompt: None,
            agent_login_shell: false,
            is_restore: false,
        },
    );

    let joined = args.join(" ");
    assert!(
        joined.contains("--session-id 11111111-2222-3333-4444-555555555555"),
        "{joined}"
    );
    assert!(!joined.contains("--resume"), "{joined}");
}
