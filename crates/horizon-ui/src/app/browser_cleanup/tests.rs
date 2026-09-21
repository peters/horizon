use egui::Rect;
use horizon_core::browser::{BrowserConfig, BrowserPanelState, BrowserStatus};
use horizon_core::{
    CanvasViewState, Panel, PanelContent, PanelId, PanelKind, PanelOptions, RuntimeState, WindowConfig, WorkspaceId,
};

use crate::app::{HorizonApp, test_support};

fn browser(app: &mut HorizonApp, workspace: WorkspaceId, id: u64, status: BrowserStatus, visible: bool) -> PanelId {
    let id = PanelId(id);
    let mut state = BrowserPanelState::restored_remote(
        format!("cleanup-test-{}", id.0),
        &BrowserConfig {
            profile_root: Some(app.session_store.home().root().join("browser-profiles")),
            ..BrowserConfig::default()
        },
        "synthetic-device".into(),
        Some("https://example.test".into()),
    );
    state.status = status;
    let mut panel = Panel::from_content(
        id,
        workspace,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(state)),
    );
    panel.visible = visible;
    app.board.panels.push(panel);
    app.board.workspace_mut(workspace).expect("workspace").add_panel(id);
    id
}

#[test]
fn host_poll_removes_ended_browsers_from_board_and_saved_state() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let stopped = browser(&mut app, workspace, 100, BrowserStatus::Stopped { code: None }, true);
    let failed = browser(
        &mut app,
        workspace,
        101,
        BrowserStatus::Error {
            message: "session failed".into(),
        },
        false,
    );
    let ready = browser(&mut app, workspace, 102, BrowserStatus::Ready, false);
    let starting = browser(&mut app, workspace, 103, BrowserStatus::Starting, true);

    assert!(app.poll_browser_create_requests());

    assert!(app.board.panel(stopped).is_none());
    assert!(app.board.panel(failed).is_none());
    assert!(!app.board.panel(ready).expect("live hidden panel").visible);
    assert!(app.board.panel(starting).is_some());
    assert_eq!(
        app.board.workspace(workspace).expect("workspace").panels,
        vec![ready, starting]
    );
    assert!(app.runtime_dirty_since.is_some());
    assert!(app.board.has_pending_browser_cleanup(), "teardown must remain tracked");
    let saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    assert_eq!(saved.workspaces[0].panels.len(), 2);
}

#[test]
fn cleanup_clears_fullscreen_and_render_state_without_closing_other_panel_kinds() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let editor = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Editor,
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("editor");
    let stopped = browser(&mut app, workspace, 100, BrowserStatus::Stopped { code: Some(1) }, true);
    app.board.focus(stopped);
    app.fullscreen_panel = Some(stopped);
    app.panel_screen_rects.insert(stopped, Rect::NOTHING);
    app.terminal_body_screen_rects.insert(stopped, Rect::NOTHING);
    app.panel_render_caches.browser_ui_state.entry(stopped).or_default();

    app.poll_browser_create_requests();

    assert_eq!(app.fullscreen_panel, None);
    assert_eq!(app.board.focused, Some(editor));
    assert!(app.board.panel(editor).is_some());
    assert!(!app.panel_screen_rects.contains_key(&stopped));
    assert!(!app.terminal_body_screen_rects.contains_key(&stopped));
    assert!(!app.panel_render_caches.browser_ui_state.contains_key(&stopped));
}

#[test]
fn failed_navigation_does_not_remove_a_live_browser() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let ready = browser(&mut app, workspace, 100, BrowserStatus::Ready, true);
    app.board
        .panel_mut(ready)
        .expect("panel")
        .browser_mut()
        .expect("browser")
        .navigation_error = Some("The page was unreachable".into());

    app.poll_browser_create_requests();

    assert!(app.board.panel(ready).is_some());
}

#[test]
fn pending_create_keeps_its_failure_until_the_request_is_completed() {
    let (_temp, mut app) = test_support::test_app();
    let workspace = app.board.create_workspace("Synthetic workspace");
    let failed = browser(
        &mut app,
        workspace,
        100,
        BrowserStatus::Error {
            message: "session failed".into(),
        },
        true,
    );
    app.mark_browser_create_pending_for_tests(crate::app::browser_requests::PendingBrowserCreateProbe {
        panel_id: failed,
        panel_local_id: app.board.panel(failed).expect("panel").local_id.clone(),
    });

    assert!(!app.close_ended_browser_panels());
    assert!(app.board.panel(failed).is_some());
    assert!(app.browser_create_is_pending(failed));
}

#[test]
fn restored_remote_sessions_are_removed_before_rendering_and_stay_out_of_saved_state() {
    use horizon_core::{BrowserProfileState, StartupDecision};

    let mut workspace = test_support::editor_workspace_state("restored", [0.0, 0.0]);
    let mut remote = workspace.panels[0].clone();
    remote.local_id = "ended-remote-browser".into();
    remote.kind = PanelKind::Browser;
    let profiles = tempfile::tempdir().expect("profiles");
    remote.browser_profile = Some(BrowserProfileState {
        root: Some(profiles.path().to_path_buf()),
        remote_target: Some("synthetic-device".into()),
        ..BrowserProfileState::default()
    });
    workspace.panels.push(remote);
    let runtime = RuntimeState {
        workspaces: vec![workspace],
        ..RuntimeState::default()
    };
    let (_temp, ctx, mut app) = test_support::test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    assert!(app.board.panel_id_by_local_id("ended-remote-browser").is_some());

    test_support::run_app_frame_with_input(&ctx, &mut app, test_support::raw_input([1400.0, 900.0], None));

    assert!(app.board.panel_id_by_local_id("ended-remote-browser").is_none());
    let saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    let saved = serde_yaml::to_string(&saved).expect("saved runtime");
    assert!(!saved.contains("ended-remote-browser"));
    assert!(!app.close_ended_browser_panels(), "cleanup is idempotent");
}
