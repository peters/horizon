//! The close of one panel of a machine, through the sidebar that a person uses.

use horizon_core::{Machine, MachineRegistry, Machines, PanelOptions, WorkspacePlace, stand_in_program};

use super::super::SidebarActions;

/// A Horizon whose workspace is placed on `build-1`, with one terminal of it,
/// which runs the stand-in program so the test starts no ssh.
fn app_with_a_machine_terminal() -> (tempfile::TempDir, crate::app::HorizonApp, horizon_core::PanelId) {
    let (temp, mut app) = crate::app::test_support::test_app();
    let workspace = app.board.create_workspace("work");
    app.board
        .workspaces
        .iter_mut()
        .find(|candidate| candidate.id == workspace)
        .expect("the workspace")
        .local_id = "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a".to_owned();
    app.board
        .set_workspace_place(workspace, WorkspacePlace::Machine("build-1".to_owned()));
    app.machine_list = Machines::with_records(
        MachineRegistry::from_home(app.session_store.home()),
        vec![Machine {
            name: "build-1".to_owned(),
            host: "203.0.113.10".to_owned(),
            ..Machine::default()
        }],
    );
    let panel = app
        .create_panel_with_options(
            PanelOptions {
                launch_wrapper: vec![stand_in_program().to_owned()],
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a terminal of the machine");
    (temp, app, panel)
}

#[test]
fn the_close_of_the_row_of_a_panel_of_a_machine_asks_before_the_session_ends() {
    let (_temp, mut app, panel) = app_with_a_machine_terminal();
    let ctx = egui::Context::default();

    // The close control of a row and the "Close" of its menu both hand this
    // action to the sidebar, which applies it here.
    let actions = SidebarActions {
        close_panel: Some(panel),
        ..SidebarActions::default()
    };
    app.apply_sidebar_actions(&ctx, &actions);
    assert!(app.board.panel(panel).is_some(), "the close of the row asks first");
    assert_eq!(app.close_panel_confirm, Some(panel), "the row asks for the one close");
    assert!(app.session_kills.is_empty(), "no session ends before the answer");

    app.answer_close_panel(panel, true);
    assert!(app.board.panel(panel).is_none(), "the answered close ends the panel");
    assert_eq!(
        app.session_kills
            .iter()
            .map(|kill| kill.session.as_str())
            .collect::<Vec<_>>(),
        ["hz-c8dfe3af-1"],
        "the answered close ends the session of the panel"
    );
}
