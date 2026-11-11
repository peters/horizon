//! Every way that a panel of each kind leaves, run inside a frame.
//!
//! The guard of `horizon_core::ui_thread` panics in strict mode, which the test
//! app turns on at its start, when a close runs a call that can wait on the UI
//! thread. Each test here closes a panel of one kind by one path, and a report
//! names the kind and the path.

use horizon_core::browser::{BrowserConfig, BrowserPanelState, BrowserStatus};
use horizon_core::{
    BlockPanelState, Panel, PanelContent, PanelId, PanelKind, PanelOptions, RuntimeState, WorkspaceId, stand_in_program,
};

use crate::app::HorizonApp;
use crate::app::panel_restarts::RestartCause;
use crate::app::test_support::test_app;

/// The kinds whose close paths this file judges.
const KINDS: [PanelKind; 5] = [
    PanelKind::Shell,
    PanelKind::Editor,
    PanelKind::Browser,
    PanelKind::Device,
    PanelKind::Block,
];

fn in_a_frame<T>(work: impl FnOnce() -> T) -> T {
    let _frame = horizon_core::ui_thread::FrameScope::enter();
    work()
}

/// The panel of `kind` that a test closes, with its workspace.
fn app_with(kind: PanelKind) -> (tempfile::TempDir, HorizonApp, WorkspaceId, PanelId, std::path::PathBuf) {
    let (temp, mut app) = test_app();
    let file = temp.path().join("notes.md");
    std::fs::write(&file, "the saved text\n").expect("the file of the editor");
    let workspace = app.board.ensure_workspace();
    let panel = match kind {
        PanelKind::Browser => restored_browser(&mut app, workspace),
        PanelKind::Device => app
            .create_panel_with_options(
                PanelOptions {
                    kind,
                    command: Some("127.0.0.1:5900".to_owned()),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("a device panel"),
        PanelKind::Block => app
            .create_panel_with_options(
                PanelOptions {
                    kind,
                    name: Some("Card".to_owned()),
                    block: Some(BlockPanelState::new(BlockPanelState::empty_document())),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("a card"),
        _ => app
            .create_panel_with_options(
                PanelOptions {
                    kind,
                    command: (kind == PanelKind::Editor).then(|| file.display().to_string()),
                    launch_wrapper: vec![stand_in_program().to_owned()],
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("a panel"),
    };
    app.settle_panel_starts();
    if kind == PanelKind::Editor {
        let editor = app
            .board
            .panel_mut(panel)
            .and_then(Panel::editor_mut)
            .expect("the editor of the panel");
        "the text of the person\n".clone_into(&mut editor.text);
        editor.dirty = true;
    }
    (temp, app, workspace, panel, file)
}

/// A stopped page, whose close deletes no profile of a running browser.
fn restored_browser(app: &mut HorizonApp, workspace: WorkspaceId) -> PanelId {
    let mut state = BrowserPanelState::restored_remote(
        "close-test".to_owned(),
        &BrowserConfig {
            profile_root: Some(app.session_store.home().root().join("browser-profiles")),
            ..BrowserConfig::default()
        },
        "synthetic-device".into(),
        Some("https://example.test".into()),
    );
    state.status = BrowserStatus::Stopped { code: None };
    let id = PanelId(900);
    let panel = Panel::from_content(
        id,
        workspace,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(state)),
    );
    app.board.panels.push(panel);
    app.board.workspace_mut(workspace).expect("the workspace").add_panel(id);
    id
}

fn assert_left(app: &HorizonApp, panel: PanelId, kind: PanelKind, path: &str) {
    assert!(
        app.board.panel(panel).is_none(),
        "{kind:?}: the close path {path} left the panel on the board"
    );
}

/// Runs the frames of an in-flight session switch, as the window would.
fn finish_session_switch(app: &mut HorizonApp) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.pending_session_switch.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the session switch did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _ = app.poll_session_switch(&ctx);
    }
}

#[test]
fn the_close_control_of_a_panel_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, mut app, _workspace, panel, file) = app_with(kind);
        in_a_frame(|| app.close_panel(panel));
        assert_left(&app, panel, kind, "the close control");
        app.settle_browser_host_io();
        if kind == PanelKind::Editor {
            assert_eq!(
                std::fs::read_to_string(&file).expect("the file of the editor"),
                "the text of the person\n",
                "the closed editor wrote its text"
            );
        }
    }
}

#[test]
fn the_close_of_a_workspace_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, mut app, workspace, panel, _file) = app_with(kind);
        in_a_frame(|| app.close_workspace_panels(workspace));
        assert_left(&app, panel, kind, "the close of its workspace");
        app.settle_browser_host_io();
    }
}

#[test]
fn a_session_switch_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, mut app, _workspace, panel, file) = app_with(kind);
        let target = app
            .session_store
            .create_session_from_runtime(RuntimeState::default())
            .expect("target session");
        in_a_frame(|| app.activate_runtime_session(&egui::Context::default(), &target));
        finish_session_switch(&mut app);
        assert_left(&app, panel, kind, "a session switch");
        app.settle_browser_host_io();
        if kind == PanelKind::Editor {
            assert_eq!(
                std::fs::read_to_string(&file).expect("the file of the editor"),
                "the text of the person\n",
                "the switch wrote the text of the editor of the board it left"
            );
        }
    }
}

#[test]
fn the_window_close_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, mut app, _workspace, panel, file) = app_with(kind);
        in_a_frame(|| app.begin_shutdown());
        in_a_frame(|| app.run_exit_cleanup());
        assert!(
            app.board.panel(panel).is_some(),
            "{kind:?}: the exit keeps its board until the window ends"
        );
        if kind == PanelKind::Editor {
            assert_eq!(
                std::fs::read_to_string(&file).expect("the file of the editor"),
                "the text of the person\n",
                "the exit wrote the text of the editor"
            );
        }
    }
}

#[test]
fn an_exit_cleanup_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, mut app, _workspace, _panel, _file) = app_with(kind);
        in_a_frame(|| app.run_exit_cleanup());
    }
}

#[test]
fn a_restart_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, mut app, _workspace, panel, _file) = app_with(kind);
        in_a_frame(|| app.request_panel_restart(panel, RestartCause::Panel));
        app.settle_browser_host_io();
        in_a_frame(|| app.close_panel(panel));
        app.settle_browser_host_io();
    }
}

#[test]
fn the_drop_of_a_board_of_each_kind_waits_for_nothing_in_a_frame() {
    for kind in KINDS {
        let (_temp, app, _workspace, _panel, _file) = app_with(kind);
        in_a_frame(|| drop(app));
    }
}
