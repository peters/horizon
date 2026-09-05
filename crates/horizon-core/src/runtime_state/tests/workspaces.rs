use super::*;

#[test]
fn from_board_preserves_window_view_focus_and_bindings() {
    let mut board = Board::new();
    let alpha = board.create_workspace_at("alpha", [120.0, 64.0]);
    let beta = board.create_workspace_at("beta", [860.0, 64.0]);
    let panel_id = board
        .create_panel(
            PanelOptions {
                name: Some("agent shell".to_string()),
                kind: PanelKind::Codex,
                resume: PanelResume::Session {
                    session_id: "session-42".to_string(),
                },
                position: Some([180.0, 120.0]),
                size: Some([640.0, 420.0]),
                session_binding: Some(AgentSessionBinding::new(
                    PanelKind::Codex,
                    "session-42".to_string(),
                    Some("/repo".to_string()),
                    Some("Codex session".to_string()),
                    Some(17),
                )),
                ..PanelOptions::default()
            },
            beta,
        )
        .expect("panel should spawn");
    board.focus_workspace(alpha);
    board.focus(panel_id);

    let window = WindowConfig {
        width: 1920.0,
        height: 1080.0,
        x: Some(32.0),
        y: Some(48.0),
    };
    let state = RuntimeState::from_board(&board, window.clone(), CanvasViewState::new([24.0, -18.0], 1.6));

    let saved_window = state.window.expect("window config");
    assert!((saved_window.width - window.width).abs() <= f32::EPSILON);
    assert!((saved_window.height - window.height).abs() <= f32::EPSILON);
    assert_eq!(saved_window.x, window.x);
    assert_eq!(saved_window.y, window.y);
    assert_eq!(state.canvas_view, Some(CanvasViewState::new([24.0, -18.0], 1.6)));
    assert_eq!(state.pan_offset, None);
    assert_eq!(
        state.active_workspace_local_id.as_deref(),
        Some(board.workspace(beta).expect("workspace").local_id.as_str())
    );
    assert_eq!(
        state.focused_panel_local_id.as_deref(),
        Some(board.panel(panel_id).expect("panel").local_id.as_str())
    );
    assert_eq!(state.workspaces.len(), 2);

    let saved_workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.local_id == board.workspace(beta).expect("workspace").local_id)
        .expect("workspace state");
    let saved_panel = saved_workspace.panels.first().expect("panel state");
    assert_eq!(saved_workspace.position, Some([860.0, 64.0]));
    assert_eq!(saved_workspace.layout, None);
    assert_eq!(saved_panel.position, Some([180.0, 120.0]));
    assert_eq!(saved_panel.size, Some([640.0, 420.0]));
    assert_eq!(saved_panel.name_is_custom, Some(true));
    assert_eq!(
        saved_panel
            .session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str()),
        Some("session-42")
    );
}

#[test]
fn from_board_persists_workspace_layout_selection() {
    let mut board = Board::new();
    let workspace_id = board.create_workspace_at("grid", [860.0, 64.0]);
    board
        .create_panel(PanelOptions::default(), workspace_id)
        .expect("first panel should spawn");
    board
        .create_panel(PanelOptions::default(), workspace_id)
        .expect("second panel should spawn");
    board.arrange_workspace(workspace_id, WorkspaceLayout::Grid);

    let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());
    let saved_workspace = state
        .workspaces
        .iter()
        .find(|workspace| workspace.local_id == board.workspace(workspace_id).expect("workspace").local_id)
        .expect("workspace state");

    assert_eq!(saved_workspace.layout, Some(WorkspaceLayout::Grid));
    board.shutdown_terminal_panels();
}

#[test]
fn workspace_state_from_config_defaults_layout_to_grid() {
    let workspace = WorkspaceConfig {
        name: "Alpha".to_string(),
        color: None,
        cwd: None,
        position: None,
        terminals: vec![TerminalConfig {
            name: "Shell".to_string(),
            ..TerminalConfig::default()
        }],
    };

    let state = WorkspaceState::from_config(0, &workspace, [120.0, 64.0]);

    assert_eq!(state.layout, Some(WorkspaceLayout::Grid));
}

#[test]
fn ensure_local_ids_repairs_duplicates_without_moving_focus() {
    let mut state = RuntimeState {
        focused_panel_local_id: Some("shared-browser".to_string()),
        workspaces: vec![WorkspaceState {
            local_id: "workspace".to_string(),
            panels: vec![
                PanelState {
                    local_id: "shared-browser".to_string(),
                    kind: PanelKind::Browser,
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "shared-browser".to_string(),
                    kind: PanelKind::Browser,
                    ..PanelState::default()
                },
                PanelState {
                    local_id: "shared-browser".to_string(),
                    ..PanelState::default()
                },
            ],
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };

    state.ensure_local_ids();

    let local_ids = state.workspaces[0]
        .panels
        .iter()
        .map(|panel| panel.local_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(local_ids[0], "shared-browser");
    assert_ne!(local_ids[1], "shared-browser");
    assert_ne!(local_ids[2], "shared-browser");
    assert_eq!(local_ids.iter().copied().collect::<HashSet<_>>().len(), 3);
    assert_eq!(state.focused_panel_local_id.as_deref(), Some("shared-browser"));
}

#[test]
fn workspace_state_from_config_uses_manual_layout_when_any_panel_has_explicit_position() {
    let workspace = WorkspaceConfig {
        name: "Alpha".to_string(),
        color: None,
        cwd: None,
        position: None,
        terminals: vec![TerminalConfig {
            name: "Shell".to_string(),
            position: Some([120.0, 80.0]),
            ..TerminalConfig::default()
        }],
    };

    let state = WorkspaceState::from_config(0, &workspace, [120.0, 64.0]);

    assert_eq!(state.layout, None);
}

#[test]
fn load_maps_removed_layout_variants_to_manual_placement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime.yaml");
    std::fs::write(
        &path,
        r"version: 2
workspaces:
  - local_id: ws-stack
    name: Stack Workspace
    layout: Stack
    panels: []
  - local_id: ws-cascade
    name: Cascade Workspace
    layout: cascade
    panels: []
",
    )
    .expect("write runtime state");

    let state = RuntimeState::load(&path)
        .expect("load runtime state")
        .expect("runtime state present");

    assert_eq!(state.workspaces.len(), 2);
    assert!(state.workspaces.iter().all(|workspace| workspace.layout.is_none()));
}

#[test]
fn load_migrates_legacy_pan_offset_into_canvas_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime.yaml");
    std::fs::write(
        &path,
        r"version: 1
pan_offset:
  - 48.0
  - -12.0
workspaces: []
",
    )
    .expect("write runtime state");

    let state = RuntimeState::load(&path)
        .expect("load runtime state")
        .expect("runtime state present");

    assert_eq!(
        state.canvas_view,
        Some(CanvasViewState::from_legacy_pan_offset([48.0, -12.0]))
    );
    assert_eq!(state.pan_offset, None);
    assert!(state.has_persisted_canvas_view());
}

#[test]
fn canvas_view_defaults_when_runtime_state_has_no_persisted_view() {
    let state = RuntimeState::default();

    assert_eq!(state.canvas_view_or_default(), CanvasViewState::default());
    assert!(!state.has_persisted_canvas_view());
}

#[test]
fn from_board_persists_detached_workspaces() {
    let mut board = Board::new();
    let alpha = board.create_workspace_at("alpha", [120.0, 64.0]);
    let beta = board.create_workspace_at("beta", [860.0, 64.0]);

    let alpha_local_id = board.workspace(alpha).expect("alpha workspace").local_id.clone();
    let beta_local_id = board.workspace(beta).expect("beta workspace").local_id.clone();

    let detached_workspaces = vec![DetachedWorkspaceState {
        workspace_local_id: beta_local_id.clone(),
        window: WindowConfig {
            width: 1440.0,
            height: 900.0,
            x: Some(2560.0),
            y: Some(80.0),
        },
    }];

    let state = RuntimeState::from_board_with_detached_workspaces(
        &board,
        WindowConfig::default(),
        CanvasViewState::new([18.0, -12.0], 1.25),
        detached_workspaces.clone(),
    );

    assert_eq!(state.detached_workspaces.len(), 1);
    assert_eq!(state.detached_workspaces[0].workspace_local_id, beta_local_id);
    assert!((state.detached_workspaces[0].window.width - 1440.0).abs() <= f32::EPSILON);
    assert!((state.detached_workspaces[0].window.height - 900.0).abs() <= f32::EPSILON);
    assert_eq!(state.detached_workspaces[0].window.x, Some(2560.0));
    assert_eq!(state.detached_workspaces[0].window.y, Some(80.0));

    assert_eq!(
        state.workspaces[0].local_id, alpha_local_id,
        "workspace ordering should remain unchanged when detached metadata is added"
    );
}

#[test]
fn to_yaml_omits_empty_detached_workspaces() {
    let yaml = RuntimeState::default().to_yaml().expect("serialize runtime state");

    assert!(!yaml.contains("detached_workspaces"));
}

#[test]
fn load_preserves_detached_workspaces() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime.yaml");
    std::fs::write(
        &path,
        r"version: 2
canvas_view:
  pan_offset:
    - 24.0
    - -12.0
  zoom: 1.5
detached_workspaces:
  - workspace_local_id: ws-beta
    window:
      width: 1440.0
      height: 900.0
      x: 2560.0
      y: 80.0
workspaces:
  - local_id: ws-alpha
    name: Alpha
    panels: []
  - local_id: ws-beta
    name: Beta
    panels: []
",
    )
    .expect("write runtime state");

    let state = RuntimeState::load(&path)
        .expect("load runtime state")
        .expect("runtime state present");

    assert_eq!(state.detached_workspaces.len(), 1);
    assert_eq!(state.detached_workspaces[0].workspace_local_id, "ws-beta");
    assert!((state.detached_workspaces[0].window.width - 1440.0).abs() <= f32::EPSILON);
    assert!((state.detached_workspaces[0].window.height - 900.0).abs() <= f32::EPSILON);
    assert_eq!(state.detached_workspaces[0].window.x, Some(2560.0));
    assert_eq!(state.detached_workspaces[0].window.y, Some(80.0));
    assert_eq!(state.canvas_view, Some(CanvasViewState::new([24.0, -12.0], 1.5)));
}
