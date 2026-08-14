use std::collections::HashSet;

use super::super::{AgentSessionBinding, PanelResume, PanelState, RuntimeState, WorkspaceState};
use super::{
    AgentSessionBootstrapCatalog, AgentSessionCatalog, AgentSessionRecord, ExactSessionResolution, PanelKind,
    codex::CodexSessions, finish_codex_load,
};
use crate::error::Error;

mod labels;
mod provider_formats;
mod provider_scoping;

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

#[test]
fn exact_codex_load_errors_are_not_reclassified_as_stale() {
    let optional = finish_codex_load(
        Err(Error::State("optional catalog failed".to_string())),
        &HashSet::new(),
    )
    .expect("optional catalog failure is best effort");
    assert!(optional.stale_binding_ids.is_empty());

    let exact = finish_codex_load(
        Err(Error::State("exact validation failed".to_string())),
        &HashSet::from(["saved-id".to_string()]),
    );
    assert!(matches!(exact, Err(Error::State(message)) if message == "exact validation failed"));
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
fn exact_or_unbound_last_panels_need_binding_bootstrap() {
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

    assert!(runtime_state.needs_agent_binding_bootstrap());
    assert!(runtime_state.needs_agent_binding_bootstrap_for(PanelKind::Claude));
    assert!(runtime_state.needs_agent_binding_bootstrap_for(PanelKind::Codex));
    for kind in [PanelKind::OpenCode, PanelKind::Pi] {
        assert!(!runtime_state.needs_agent_binding_bootstrap_for(kind));
    }
}

#[test]
fn provider_catalog_presence_includes_bound_and_fresh_panels() {
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                PanelState {
                    kind: PanelKind::OpenCode,
                    resume: PanelResume::Session {
                        session_id: "open-pinned".to_string(),
                    },
                    ..PanelState::default()
                },
                PanelState {
                    kind: PanelKind::Pi,
                    resume: PanelResume::Fresh,
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert!(AgentSessionCatalog::has_provider_panel(
        &runtime_state,
        PanelKind::OpenCode
    ));
    assert!(AgentSessionCatalog::has_provider_panel(&runtime_state, PanelKind::Pi));
    assert!(!AgentSessionCatalog::has_provider_panel(
        &runtime_state,
        PanelKind::Claude
    ));
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
fn bootstrap_assigns_scoped_groups_before_cwd_less_groups() {
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                PanelState {
                    local_id: "unscoped".to_string(),
                    name: "Unscoped Claude".to_string(),
                    kind: PanelKind::Claude,
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "scoped".to_string(),
                    name: "Scoped Claude".to_string(),
                    kind: PanelKind::Claude,
                    cwd: Some("/repo".to_string()),
                    resume: PanelResume::Last,
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let catalog = bootstrap_catalog(
        vec![
            AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "repo-session".to_string(),
                cwd: Some("/repo".to_string()),
                label: None,
                updated_at: 2,
                interactive: true,
            },
            AgentSessionRecord {
                kind: PanelKind::Claude,
                session_id: "other-session".to_string(),
                cwd: Some("/other".to_string()),
                label: None,
                updated_at: 1,
                interactive: true,
            },
        ],
        [],
    );

    state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new());

    let panels = &state.workspaces[0].panels;
    assert_eq!(panels[0].stored_session_id(), Some("other-session"));
    assert_eq!(panels[1].stored_session_id(), Some("repo-session"));
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
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Claude,
                    "session-live".to_string(),
                    Some("/repo".to_string()),
                    None,
                    None,
                )),
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
    assert!(matches!(panel.resume, PanelResume::Fresh));
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
    assert!(matches!(state.workspaces[0].panels[0].resume, PanelResume::Last));
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
    assert!(panels[0].session_binding.is_some());
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
    assert!(panel.session_binding.is_some());
    assert!(matches!(panel.resume, PanelResume::Last));
}

#[test]
fn bootstrap_discards_a_stale_exact_binding_without_pinning_last() {
    let catalog = bootstrap_catalog(
        Vec::new(),
        [(
            (PanelKind::Codex, "archived-session".to_string()),
            ExactSessionResolution::Stale,
        )],
    );
    let mut state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![PanelState {
                kind: PanelKind::Codex,
                resume: PanelResume::Last,
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Codex,
                    "archived-session".to_string(),
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

    assert!(state.bootstrap_missing_agent_bindings(&catalog, &HashSet::new()));

    let panel = &state.workspaces[0].panels[0];
    assert!(panel.session_binding.is_none());
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
    assert_eq!(panels[2].stored_session_id(), Some("other-session"));
}
