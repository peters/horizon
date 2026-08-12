use std::collections::HashSet;
use std::io::Cursor;

use rusqlite::Connection;
use uuid::Uuid;

use super::super::{AgentSessionBinding, PanelResume, PanelState, RuntimeState, WorkspaceState};
use super::{
    AgentSessionBootstrapCatalog, AgentSessionCatalog, AgentSessionRecord, ClaudeSessionSummary,
    ExactSessionResolution, PanelKind, PiSessionSummary, codex::CodexSessions, load_claude_project_session_summary,
    load_opencode_sessions_from_path, load_pi_sessions_from_dir, scan_claude_session_reader, scan_pi_session_reader,
};
use crate::error::Error;

#[test]
fn strict_catalog_load_propagates_provider_errors() {
    let loaded = AgentSessionCatalog::load_strict(
        || Ok(Vec::new()),
        || Ok(CodexSessions::default()),
        || Err(Error::State("OpenCode store unavailable".to_string())),
        || Ok(Vec::new()),
    );

    assert!(matches!(loaded, Err(Error::State(message)) if message == "OpenCode store unavailable"));
}

fn parse_claude_project_session<R: std::io::BufRead>(
    reader: R,
    fallback_session_id: &str,
    fallback_updated_at: i64,
) -> Option<AgentSessionRecord> {
    let mut summary = ClaudeSessionSummary::default();
    scan_claude_session_reader(reader, None, &mut summary);
    summary.into_record(fallback_session_id, fallback_updated_at)
}

fn parse_pi_session<R: std::io::BufRead>(
    reader: R,
    fallback_session_id: &str,
    fallback_updated_at: i64,
) -> Option<AgentSessionRecord> {
    let mut summary = PiSessionSummary::default();
    scan_pi_session_reader(reader, None, &mut summary);
    summary.into_record(fallback_session_id, fallback_updated_at)
}

fn bootstrap_catalog(
    sessions: Vec<AgentSessionRecord>,
    exact_resolutions: impl IntoIterator<Item = ((PanelKind, String), ExactSessionResolution)>,
) -> AgentSessionBootstrapCatalog {
    AgentSessionBootstrapCatalog {
        catalog: AgentSessionCatalog { sessions },
        exact_resolutions: exact_resolutions.into_iter().collect(),
        unavailable_exact_session_ids: HashSet::new(),
    }
}

#[test]
fn only_unbound_last_panels_need_recent_provider_sessions() {
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                PanelState {
                    kind: PanelKind::Claude,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
                PanelState {
                    kind: PanelKind::Codex,
                    resume: PanelResume::Last,
                    session_binding: Some(AgentSessionBinding::new(
                        PanelKind::Codex,
                        "session-root".to_string(),
                        None,
                        None,
                        None,
                    )),
                    ..PanelState::default()
                },
                PanelState {
                    kind: PanelKind::OpenCode,
                    resume: PanelResume::Fresh,
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(AgentSessionCatalog::needs_recent_for_runtime_state(
        &runtime_state,
        PanelKind::Claude
    ));
    for kind in [PanelKind::Codex, PanelKind::OpenCode, PanelKind::Pi] {
        assert!(!AgentSessionCatalog::needs_recent_for_runtime_state(
            &runtime_state,
            kind
        ));
    }
}

#[test]
fn bootstrap_assigns_distinct_sessions_per_group() {
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "termgalore".to_string(),
            cwd: Some("/repo".to_string()),
            position: None,
            template: None,
            layout: None,
            panels: vec![
                PanelState {
                    local_id: "a".to_string(),
                    name: "Claude A".to_string(),
                    kind: PanelKind::Claude,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "b".to_string(),
                    name: "Claude B".to_string(),
                    kind: PanelKind::Claude,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
            ],
        }],
        ..RuntimeState::default()
    };
    let catalog = bootstrap_catalog(
        vec![
            AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "session-1".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 2,
                interactive: true,
            },
            AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "session-2".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 1,
                interactive: true,
            },
        ],
        [],
    );

    state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new());

    let bindings: Vec<_> = state.workspaces[0]
        .panels
        .iter()
        .filter_map(|panel| panel.session_binding.as_ref().map(|binding| binding.session_id.clone()))
        .collect();
    assert_eq!(bindings.len(), 2);
    assert_ne!(bindings[0], bindings[1]);
}

#[test]
fn bootstrap_never_assigns_sessions_open_in_other_processes() {
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "termgalore".to_string(),
            cwd: Some("/repo".to_string()),
            position: None,
            template: None,
            layout: None,
            panels: vec![PanelState {
                local_id: "a".to_string(),
                name: "Claude A".to_string(),
                kind: PanelKind::Claude,
                cwd: Some("/repo".to_string()),
                resume: PanelResume::Last,
                ..PanelState::default()
            }],
        }],
        ..RuntimeState::default()
    };
    let catalog = bootstrap_catalog(
        vec![
            AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "session-live".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 2,
                interactive: true,
            },
            AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "session-free".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 1,
                interactive: true,
            },
        ],
        [],
    );
    let busy = HashSet::from(["session-live".to_string()]);

    state.bootstrap_missing_agent_bindings(&catalog, &busy);

    let binding = state.workspaces[0].panels[0]
        .session_binding
        .as_ref()
        .map(|binding| binding.session_id.clone());
    assert_eq!(binding.as_deref(), Some("session-free"));
}

#[test]
fn bootstrap_repairs_persisted_codex_child_bindings() {
    let root = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-root".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Root session".to_string()),
        updated_at: 42,
        interactive: true,
    };
    let fallback = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-fallback".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Fallback session".to_string()),
        updated_at: 41,
        interactive: true,
    };
    let catalog = bootstrap_catalog(
        vec![root.clone(), fallback],
        [(
            (PanelKind::Codex, "session-child".to_string()),
            ExactSessionResolution::Rebind(root.into_binding()),
        )],
    );
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "horizon".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                cwd: Some("/repo".to_string()),
                resume: PanelResume::Fresh,
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Codex,
                    "session-child".to_string(),
                    Some("/repo".to_string()),
                    None,
                    Some(50),
                )),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    let changed = state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new());

    assert!(changed);
    let panel = &state.workspaces[0].panels[0];
    assert_eq!(
        panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-root")
    );
    assert_eq!(
        panel
            .session_binding
            .as_ref()
            .and_then(|binding| binding.label.as_deref()),
        Some("Root session")
    );
    assert!(matches!(
        &panel.resume,
        PanelResume::Session { session_id } if session_id == "session-root"
    ));
}

#[test]
fn bootstrap_repairs_an_explicit_codex_child_resume() {
    let root = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-root".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Root session".to_string()),
        updated_at: 42,
        interactive: true,
    };
    let fallback = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-fallback".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Fallback session".to_string()),
        updated_at: 41,
        interactive: true,
    };
    let catalog = bootstrap_catalog(
        vec![root.clone(), fallback],
        [(
            (PanelKind::Codex, "session-child".to_string()),
            ExactSessionResolution::Rebind(root.into_binding()),
        )],
    );
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "horizon".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                cwd: Some("/repo".to_string()),
                resume: PanelResume::Session {
                    session_id: "session-child".to_string(),
                },
                session_binding: None,
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

    let panel = &state.workspaces[0].panels[0];
    assert_eq!(
        panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-root")
    );
    assert!(matches!(
        &panel.resume,
        PanelResume::Session { session_id } if session_id == "session-root"
    ));
}

#[test]
fn bootstrap_does_not_duplicate_a_root_resume() {
    let root = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-root".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Root session".to_string()),
        updated_at: 42,
        interactive: true,
    };
    let fallback = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-fallback".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Fallback session".to_string()),
        updated_at: 41,
        interactive: true,
    };
    let catalog = bootstrap_catalog(
        vec![root.clone(), fallback],
        [
            (
                (PanelKind::Codex, "session-root".to_string()),
                ExactSessionResolution::Verified,
            ),
            (
                (PanelKind::Codex, "session-child-a".to_string()),
                ExactSessionResolution::Rebind(root.clone().into_binding()),
            ),
            (
                (PanelKind::Codex, "session-child-b".to_string()),
                ExactSessionResolution::Rebind(root.into_binding()),
            ),
        ],
    );
    let binding = |session_id: &str| {
        AgentSessionBinding::new(
            PanelKind::Codex,
            session_id.to_string(),
            Some("/repo".to_string()),
            None,
            Some(50),
        )
    };
    let panel = |local_id: &str, session_id: &str| PanelState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        kind: PanelKind::Codex,
        cwd: Some("/repo".to_string()),
        resume: PanelResume::Last,
        session_binding: Some(binding(session_id)),
        ..PanelState::default()
    };
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "horizon".to_string(),
            panels: vec![
                panel("root", "session-root"),
                panel("child-a", "session-child-a"),
                panel("child-b", "session-child-b"),
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

    assert_eq!(
        state.workspaces[0].panels[0]
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-root")
    );
    let children = &state.workspaces[0].panels[1..];
    assert_eq!(
        children[0]
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-fallback")
    );
    assert!(children[1].session_binding.is_none());
    assert!(children.iter().all(|child| matches!(child.resume, PanelResume::Last)));
}

#[test]
fn bootstrap_retains_the_later_duplicate_direct_root_without_resuming_it() {
    let catalog = bootstrap_catalog(
        Vec::new(),
        [(
            (PanelKind::Codex, "session-root".to_string()),
            ExactSessionResolution::Verified,
        )],
    );
    let panel = |local_id: &str| PanelState {
        local_id: local_id.to_string(),
        name: local_id.to_string(),
        kind: PanelKind::Codex,
        resume: PanelResume::Last,
        session_binding: Some(AgentSessionBinding::new(
            PanelKind::Codex,
            "session-root".to_string(),
            Some("/repo".to_string()),
            None,
            None,
        )),
        ..PanelState::default()
    };
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![panel("first"), panel("second")],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

    let panels = &state.workspaces[0].panels;
    assert!(
        panels[0]
            .session_binding
            .as_ref()
            .is_some_and(|binding| binding.resumable)
    );
    assert!(panels[1].session_binding.is_none());
    assert_eq!(panels[1].stored_session_id(), None);
    assert!(matches!(panels[1].resume, PanelResume::Last));
}

#[test]
fn bootstrap_retains_an_unresolved_codex_child_binding() {
    let catalog = bootstrap_catalog(
        Vec::new(),
        [(
            (PanelKind::Codex, "session-child".to_string()),
            ExactSessionResolution::Unavailable,
        )],
    );
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "horizon".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                cwd: Some("/repo".to_string()),
                resume: PanelResume::Session {
                    session_id: "session-child".to_string(),
                },
                session_binding: None,
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(!state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

    let panel = &state.workspaces[0].panels[0];
    assert_eq!(panel.stored_session_id(), Some("session-child"));
    assert!(panel.session_binding.is_none());
    assert!(matches!(
        &panel.resume,
        PanelResume::Session { session_id } if session_id == "session-child"
    ));
}

#[test]
fn bootstrap_preserves_last_for_an_unresolved_codex_child_binding() {
    let fallback = AgentSessionRecord {
        kind: PanelKind::Codex,
        session_id: "session-fallback".to_string(),
        cwd: Some("/repo".to_string()),
        label: Some("Fallback session".to_string()),
        updated_at: 41,
        interactive: true,
    };
    let catalog = bootstrap_catalog(
        vec![fallback],
        [(
            (PanelKind::Codex, "session-child".to_string()),
            ExactSessionResolution::Unavailable,
        )],
    );
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "horizon".to_string(),
            panels: vec![PanelState {
                local_id: "panel".to_string(),
                name: "Codex".to_string(),
                kind: PanelKind::Codex,
                cwd: Some("/repo".to_string()),
                resume: PanelResume::Last,
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Codex,
                    "session-child".to_string(),
                    Some("/repo".to_string()),
                    None,
                    None,
                )),
                ..PanelState::default()
            }],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(!state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

    let panel = &state.workspaces[0].panels[0];
    assert_eq!(
        panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-child")
    );
    assert!(panel.session_binding.as_ref().is_some_and(|binding| binding.resumable));
    assert!(matches!(panel.resume, PanelResume::Last));
}

#[test]
fn explicit_recovery_neutralizes_only_scoped_unverified_ids() {
    let binding = AgentSessionBinding::new(
        PanelKind::Codex,
        "session-child".to_string(),
        Some("/repo".to_string()),
        None,
        None,
    );
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            name: "horizon".to_string(),
            panels: vec![
                PanelState {
                    local_id: "last".to_string(),
                    name: "Last".to_string(),
                    kind: PanelKind::Codex,
                    resume: PanelResume::Last,
                    session_binding: Some(binding),
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "explicit".to_string(),
                    name: "Explicit".to_string(),
                    kind: PanelKind::Codex,
                    resume: PanelResume::Session {
                        session_id: "session-missing".to_string(),
                    },
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "other".to_string(),
                    name: "Other".to_string(),
                    kind: PanelKind::OpenCode,
                    resume: PanelResume::Session {
                        session_id: "other-session".to_string(),
                    },
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(state.neutralize_unverified_session_bindings(&HashSet::from([
        "session-child".to_string(),
        "session-missing".to_string(),
    ])));

    let panels = &state.workspaces[0].panels;
    assert_eq!(panels[0].stored_session_id(), None);
    assert!(panels[0].session_binding.is_none());
    assert!(matches!(panels[0].resume, PanelResume::Last));
    assert_eq!(panels[1].stored_session_id(), None);
    assert!(panels[1].session_binding.is_none());
    assert!(matches!(panels[1].resume, PanelResume::Fresh));
    assert_eq!(panels[2].exact_session_id(), Some("other-session"));
}

#[test]
fn parse_claude_project_session_uses_resumable_jsonl_session_id() {
    let jsonl = concat!(
        "{\"type\":\"user\",\"cwd\":\"/repo\",\"sessionId\":\"session-123\",\"slug\":\"quiet-river\"}\n",
        "{\"type\":\"last-prompt\",\"lastPrompt\":\"reply with ok only\",\"sessionId\":\"session-123\"}\n",
    );

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 42).expect("session");

    assert_eq!(session.kind, PanelKind::Claude);
    assert_eq!(session.session_id, "session-123");
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.label.as_deref(), Some("reply with ok only"));
    assert_eq!(session.updated_at, 42);
}

#[test]
fn parse_claude_project_session_falls_back_to_filename_id() {
    let jsonl = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n";

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert_eq!(session.session_id, "fallback-id");
    assert_eq!(session.cwd, None);
    assert_eq!(session.label.as_deref(), Some("Claude session"));
    assert_eq!(session.updated_at, 7);
}

#[test]
fn parse_claude_project_session_keeps_parent_uuid_without_sidechain_interactive() {
    let jsonl = "{\"type\":\"assistant\",\"sessionId\":\"session-123\",\"parentUuid\":\"message-1\"}\n";

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(session.interactive);
}

#[test]
fn parse_claude_project_session_keeps_parent_uuid_with_false_sidechain_interactive() {
    let jsonl = concat!(
        "{\"type\":\"assistant\",\"sessionId\":\"session-123\",",
        "\"isSidechain\":false,\"parentUuid\":\"message-1\"}\n",
    );

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(session.interactive);
}

#[test]
fn parse_claude_project_session_marks_sidechains_noninteractive() {
    let jsonl = "{\"type\":\"user\",\"sessionId\":\"child\",\"isSidechain\":true,\"parentUuid\":\"root\"}\n";

    let session = parse_claude_project_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(!session.interactive);
}

#[test]
fn load_claude_project_session_summary_reads_head_and_tail_metadata() {
    let path = std::env::temp_dir().join(format!("horizon-claude-session-{}.jsonl", Uuid::new_v4()));
    let mut content =
        String::from("{\"type\":\"user\",\"cwd\":\"/repo\",\"sessionId\":\"session-123\",\"slug\":\"quiet-river\"}\n");
    for _ in 0..80 {
        content.push_str("{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\"}}\n");
    }
    content
        .push_str("{\"type\":\"last-prompt\",\"lastPrompt\":\"reply with ok only\",\"sessionId\":\"session-123\"}\n");
    std::fs::write(&path, content).expect("write temp session file");

    let session = load_claude_project_session_summary(&path, 9)
        .expect("load")
        .expect("session");
    std::fs::remove_file(&path).ok();

    assert_eq!(session.kind, PanelKind::Claude);
    assert_eq!(session.session_id, "session-123");
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.label.as_deref(), Some("reply with ok only"));
    assert_eq!(session.updated_at, 9);
}

#[test]
fn load_opencode_sessions_reads_root_sessions_from_sqlite() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let sqlite_path = temp_dir.path().join("opencode.db");
    let conn = Connection::open(&sqlite_path).expect("sqlite");
    conn.execute_batch(
        "\
CREATE TABLE session (
id TEXT PRIMARY KEY,
title TEXT NOT NULL,
directory TEXT NOT NULL,
parent_id TEXT,
time_updated INTEGER NOT NULL,
time_archived INTEGER
);
INSERT INTO session (id, title, directory, parent_id, time_updated, time_archived) VALUES
('session-root', 'Fix auth flow', '/repo', NULL, 1000, NULL),
('session-child', 'Child', '/repo', 'session-root', 2000, NULL),
('session-archived', 'Archived', '/repo', NULL, 3000, 1),
('session-other', 'Other repo', '/other', NULL, 4000, NULL);
",
    )
    .expect("seed");

    let sessions = load_opencode_sessions_from_path(&sqlite_path).expect("opencode sessions");

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].kind, PanelKind::OpenCode);
    assert_eq!(sessions[0].session_id, "session-other");
    assert_eq!(sessions[0].cwd.as_deref(), Some("/other"));
    assert_eq!(sessions[1].session_id, "session-root");
    assert_eq!(sessions[1].cwd.as_deref(), Some("/repo"));
}

#[test]
fn parse_pi_session_uses_header_metadata_and_latest_user_message() {
    let jsonl = concat!(
        "{\"type\":\"session\",\"id\":\"pi-session-123\",\"cwd\":\"/repo\"}\n",
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"first prompt\"}]}}\n",
        "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"working\"}}\n",
        "{\"type\":\"user_message\",\"text\":\"latest prompt\"}\n",
    );

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 42).expect("session");

    assert_eq!(session.kind, PanelKind::Pi);
    assert_eq!(session.session_id, "pi-session-123");
    assert_eq!(session.cwd.as_deref(), Some("/repo"));
    assert_eq!(session.label.as_deref(), Some("latest prompt"));
    assert_eq!(session.updated_at, 42);
}

#[test]
fn parse_pi_session_falls_back_to_filename_id_and_default_label() {
    let jsonl = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":\"ok\"}}\n";

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert_eq!(session.kind, PanelKind::Pi);
    assert_eq!(session.session_id, "fallback-id");
    assert_eq!(session.cwd, None);
    assert_eq!(session.label.as_deref(), Some("Pi session"));
    assert_eq!(session.updated_at, 7);
}

#[test]
fn parse_pi_session_marks_parent_sessions_noninteractive() {
    let jsonl = "{\"type\":\"session\",\"id\":\"child\",\"parent_id\":\"root\"}\n";

    let session = parse_pi_session(Cursor::new(jsonl), "fallback-id", 7).expect("session");

    assert!(!session.interactive);
}

#[test]
fn load_pi_sessions_recurses_and_filters_by_cwd() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let nested = temp_dir.path().join("project/subdir");
    std::fs::create_dir_all(&nested).expect("create nested session dir");
    std::fs::write(
        nested.join("pi-session-123.jsonl"),
        concat!(
            "{\"session_id\":\"pi-session-123\",\"metadata\":{\"cwd\":\"/repo\"}}\n",
            "{\"role\":\"user\",\"content\":\"Fix the build\"}\n",
        ),
    )
    .expect("write pi session");
    std::fs::write(
        temp_dir.path().join("pi-session-other.jsonl"),
        concat!(
            "{\"session_id\":\"pi-session-other\",\"cwd\":\"/other\"}\n",
            "{\"role\":\"user\",\"content\":\"Other repo\"}\n",
        ),
    )
    .expect("write other pi session");

    let sessions = load_pi_sessions_from_dir(temp_dir.path()).expect("pi sessions");
    let catalog = AgentSessionCatalog { sessions };
    let repo_sessions = catalog.recent_for(PanelKind::Pi, Some("/repo"));

    assert_eq!(repo_sessions.len(), 1);
    assert_eq!(repo_sessions[0].session_id, "pi-session-123");
    assert_eq!(repo_sessions[0].label.as_deref(), Some("Fix the build"));
    assert!(catalog.recent_for(PanelKind::Pi, Some("/missing")).is_empty());
    assert!(catalog.recent_for(PanelKind::Claude, Some("/repo")).is_empty());
}
