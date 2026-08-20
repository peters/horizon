use super::*;

fn panel_state(kind: PanelKind, resume: PanelResume, session_binding: Option<AgentSessionBinding>) -> PanelState {
    PanelState {
        kind,
        resume,
        session_binding,
        ..PanelState::default()
    }
}

fn binding(kind: PanelKind, session_id: &str) -> AgentSessionBinding {
    AgentSessionBinding::new(kind, session_id.to_string(), None, None, None)
}

#[test]
fn resumable_agent_panel_count_ignores_non_agent_and_unresumable_kinds() {
    let runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                panel_state(PanelKind::Pi, PanelResume::Fresh, None),
                panel_state(PanelKind::Pi, PanelResume::Fresh, Some(binding(PanelKind::Pi, "pi-1"))),
                panel_state(PanelKind::KiloCode, PanelResume::Fresh, None),
                panel_state(PanelKind::Gemini, PanelResume::Fresh, None),
                panel_state(PanelKind::Shell, PanelResume::Fresh, None),
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    assert_eq!(runtime_state.resumable_agent_panel_count(), 3);
}

#[test]
fn apply_resume_all_flips_only_fresh_unbound_agent_panels_to_last() {
    let mut runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                panel_state(PanelKind::Pi, PanelResume::Fresh, None),
                panel_state(
                    PanelKind::Claude,
                    PanelResume::Fresh,
                    Some(binding(PanelKind::Claude, "cc-1")),
                ),
                panel_state(PanelKind::Codex, PanelResume::Last, None),
                panel_state(
                    PanelKind::OpenCode,
                    PanelResume::Session {
                        session_id: "oc-1".to_string(),
                    },
                    None,
                ),
                panel_state(PanelKind::Gemini, PanelResume::Fresh, None),
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    let changed = runtime_state.apply_resume_all_agent_panels();

    assert_eq!(changed, 1);
    let panels = &runtime_state.workspaces[0].panels;
    assert!(matches!(panels[0].resume, PanelResume::Last));
    assert!(matches!(panels[1].resume, PanelResume::Fresh));
    assert!(panels[1].session_binding.is_some());
    assert!(matches!(panels[2].resume, PanelResume::Last));
    assert!(matches!(
        panels[3].resume,
        PanelResume::Session {
            ref session_id
        } if session_id == "oc-1"
    ));
    assert!(matches!(panels[4].resume, PanelResume::Fresh));

    // The flipped Pi panel must now be picked up by the startup binding
    // bootstrap, which assigns it a catalog session before launch.
    assert!(panels[0].session_binding.is_none());
    assert!(runtime_state.needs_agent_binding_bootstrap_for(PanelKind::Pi));
}

#[test]
fn start_agent_panels_fresh_clears_bindings_and_resumes() {
    let mut runtime_state = RuntimeState {
        workspaces: vec![WorkspaceState {
            panels: vec![
                panel_state(PanelKind::Pi, PanelResume::Fresh, Some(binding(PanelKind::Pi, "pi-1"))),
                panel_state(PanelKind::Codex, PanelResume::Last, None),
                panel_state(
                    PanelKind::OpenCode,
                    PanelResume::Session {
                        session_id: "oc-1".to_string(),
                    },
                    Some(binding(PanelKind::OpenCode, "oc-1")),
                ),
                panel_state(PanelKind::Gemini, PanelResume::Fresh, None),
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    let changed = runtime_state.start_agent_panels_fresh();

    assert_eq!(changed, 3);
    let panels = &runtime_state.workspaces[0].panels;
    for panel in panels.iter().take(3) {
        assert!(panel.session_binding.is_none());
        assert!(matches!(panel.resume, PanelResume::Fresh));
    }
    assert!(matches!(panels[3].resume, PanelResume::Fresh));
}
