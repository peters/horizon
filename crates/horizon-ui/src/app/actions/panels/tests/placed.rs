//! A workspace that is placed on a machine, judged on the terminals that a
//! person makes in it.

use horizon_core::{
    Machine, MachineKeys, MachineRegistry, Machines, PanelKind, PanelOptions, RuntimeState, StartupDecision,
    WorkspaceId, WorkspacePlace, WorkspaceState, stand_in_program,
};

use crate::app::HorizonApp;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::test_app_with_startup;

/// The workspace local id of the test, as Horizon makes one: its first eight
/// hexadecimal characters name the tmux sessions of its panels.
const WORKSPACE: &str = "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a";

fn record(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        user: Some("root".to_owned()),
        key: Some("build-1".to_owned()),
        ..Machine::default()
    }
}

/// A Horizon whose one workspace is in `place`, and a machine list that keeps
/// `build-1` with a machine key.
fn app_in(place: WorkspacePlace) -> (tempfile::TempDir, HorizonApp, WorkspaceId) {
    let runtime = RuntimeState {
        workspaces: vec![WorkspaceState {
            local_id: WORKSPACE.to_owned(),
            name: "work".to_owned(),
            place,
            ..WorkspaceState::default()
        }],
        ..RuntimeState::default()
    };
    let (temp, _ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    settle_machine_load(&mut app);
    app.machine_list = Machines::with_records(
        MachineRegistry::from_home(app.session_store.home()),
        vec![record("build-1")],
    );
    let workspace = app.board.workspaces[0].id;
    (temp, app, workspace)
}

/// The options of a panel that runs the stand-in program instead of its own
/// program, so a test starts no ssh. The panel's own saved command line is what
/// it reads.
fn harmless(mut options: PanelOptions) -> PanelOptions {
    options.launch_wrapper = vec![stand_in_program().to_owned()];
    options
}

fn ssh_args(options: &PanelOptions) -> Vec<String> {
    options
        .ssh_connection
        .as_ref()
        .expect("a placed terminal connects over ssh")
        .to_command_args()
}

#[test]
fn a_placed_workspace_asks_for_no_local_directory() {
    use super::super::asks_where_to_run;
    assert!(
        asks_where_to_run(true, false, true),
        "a shell on this computer asks where it runs when its directory is gone"
    );
    assert!(
        !asks_where_to_run(true, true, true),
        "an existing directory asks nothing"
    );
    assert!(!asks_where_to_run(true, false, false), "a browser needs no directory");
    assert!(
        !asks_where_to_run(false, false, true),
        "a terminal of a machine starts where the machine puts it, so no local directory is asked"
    );
}

#[test]
fn a_shell_panel_of_a_placed_workspace_runs_ssh_into_a_session_of_its_own() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    let options = app
        .placed_options(PanelOptions::default(), workspace)
        .expect("the machine of the place accepts the terminal");

    assert_eq!(options.kind, PanelKind::Ssh, "the terminal runs on the machine");
    assert_eq!(options.machine.as_deref(), Some("build-1"));
    assert_eq!(options.machine_session.as_deref(), Some("hz-c8dfe3af-1"));

    let directory = MachineKeys::from_home(app.session_store.home())
        .key_directory()
        .display()
        .to_string();
    let args = ssh_args(&options);
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-i".to_owned(), format!("{directory}/build-1")]),
        "the terminal offers the key of the machine: {args:?}"
    );
    assert!(args.iter().any(|argument| argument == "IdentitiesOnly=yes"));
    assert!(
        args.iter()
            .any(|argument| argument == &format!("UserKnownHostsFile={directory}/build-1.known_hosts")),
        "the terminal keeps the known hosts of that key alone: {args:?}"
    );
    assert_eq!(args.iter().filter(|argument| *argument == "-t").count(), 1, "{args:?}");
    assert_eq!(args[args.len() - 2], "root@203.0.113.10");
    assert_eq!(
        args.last().map(String::as_str),
        Some("tmux new-session -A -s hz-c8dfe3af-1 \\; set-option status off"),
        "the panel attaches to its own session and keeps the status line out of it"
    );
}

#[test]
fn a_workspace_on_this_computer_makes_the_shell_of_the_person() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Local);
    let options = app
        .placed_options(PanelOptions::default(), workspace)
        .expect("this computer accepts the terminal");
    assert_eq!(options.kind, PanelKind::Shell);
    assert!(options.ssh_connection.is_none());
    assert!(options.machine_session.is_none());
}

#[test]
fn the_person_path_makes_the_ssh_panel_and_the_next_terminal_takes_the_next_session() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));

    let first = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("the first terminal of the place");
    let panel = app.board.panel(first).expect("the panel");
    assert_eq!(panel.kind, PanelKind::Ssh, "the person's New button made an SSH panel");
    assert_eq!(panel.machine(), Some("build-1"));
    assert_eq!(panel.machine_session(), Some("hz-c8dfe3af-1"));
    assert_eq!(
        panel
            .ssh_connection
            .as_ref()
            .and_then(|connection| connection.remote_command.as_deref()),
        Some("tmux new-session -A -s hz-c8dfe3af-1 \\; set-option status off")
    );

    let second = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("the second terminal of the place");
    assert_eq!(
        app.board.panel(second).and_then(horizon_core::Panel::machine_session),
        Some("hz-c8dfe3af-2"),
        "the second terminal takes the next session, not the first one again"
    );
}

#[test]
fn another_workspace_on_the_same_machine_starts_its_own_sessions() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    let _first = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of the placed workspace");

    let other = app.board.create_workspace("other");
    // A local id of this test, so the session of the second workspace is known.
    app.board
        .workspaces
        .iter_mut()
        .find(|candidate| candidate.id == other)
        .expect("the new workspace")
        .local_id = "0123abcd-1111-4889-99ed-403b4a5a4c8a".to_owned();
    app.board
        .set_workspace_place(other, WorkspacePlace::Machine("build-1".to_owned()));
    let options = app
        .placed_options(PanelOptions::default(), other)
        .expect("the machine accepts the terminal");
    assert_eq!(
        options.machine_session.as_deref(),
        Some("hz-0123abcd-1"),
        "two workspaces on one machine never share a session"
    );
}

#[test]
fn changing_the_place_leaves_the_open_panels_and_changes_the_next_one() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    let placed = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of the machine");

    assert!(app.board.set_workspace_place(workspace, WorkspacePlace::Local));
    assert_eq!(
        app.board.panel(placed).and_then(horizon_core::Panel::machine_session),
        Some("hz-c8dfe3af-1"),
        "the panel that is open stays on the machine it runs on"
    );

    let local = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of this computer");
    let panel = app.board.panel(local).expect("the panel");
    assert_eq!(panel.kind, PanelKind::Shell, "a new terminal follows the new place");
    assert!(panel.machine_session().is_none());
}

#[test]
fn a_new_terminal_of_a_place_whose_machine_is_gone_says_so_and_makes_no_panel() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    // The person forgot the machine, or an agent deleted the record.
    app.machine_list = Machines::with_records(MachineRegistry::from_home(app.session_store.home()), Vec::new());
    let before = app.board.panels.len();

    let error = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect_err("the machine of the place is gone");
    assert!(matches!(error, horizon_core::Error::Place(_)), "{error}");
    assert!(error.to_string().contains("build-1"), "{error}");
    assert_eq!(app.board.panels.len(), before, "no panel is made");
    let notice = app.sidebar_notice.as_deref().unwrap_or_default();
    assert!(
        notice.starts_with("Shell panel: "),
        "the notice names the kind of panel that opened nothing: {notice}"
    );
    assert!(
        notice.contains("machine_unavailable"),
        "the sidebar says the typed reason: {notice}"
    );
}

#[test]
fn closing_a_panel_asks_for_its_session_once_with_the_transport_of_the_panel() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    let panel_id = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of the machine");
    let panel = app.board.panel(panel_id).expect("the panel");
    let running = panel.ssh_connection.as_ref().expect("the connection").to_command_args();
    let kill = panel.session_kill().expect("the panel holds a session");

    app.close_panel(panel_id);
    assert_eq!(
        app.session_kills.len(),
        1,
        "a close asks for one kill, not none and not two"
    );
    let asked = &app.session_kills[0];
    assert_eq!(asked.session, "hz-c8dfe3af-1");
    assert_eq!(asked.program, "ssh");
    assert_eq!(asked.args.len(), running.len());
    assert_eq!(&asked.args[..asked.args.len() - 1], &running[..running.len() - 1]);
    assert_eq!(
        asked.args.last().map(String::as_str),
        Some("tmux kill-session -t hz-c8dfe3af-1"),
        "the kill runs the argv of the panel with the kill in place of the attach"
    );
    assert_eq!(asked, &kill);

    app.close_panel(panel_id);
    assert_eq!(
        app.session_kills.len(),
        1,
        "a panel that is gone asks for nothing again"
    );
}

#[test]
fn closing_a_workspace_asks_for_the_session_of_each_panel_of_its_machine() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    for _ in 0..2 {
        app.create_panel_with_options(harmless(PanelOptions::default()), workspace)
            .expect("a terminal of the machine");
    }
    app.close_workspace_panels(workspace);
    assert_eq!(
        app.session_kills
            .iter()
            .map(|kill| kill.session.as_str())
            .collect::<Vec<_>>(),
        ["hz-c8dfe3af-1", "hz-c8dfe3af-2"],
        "each panel of the workspace ends the session it named"
    );
}

#[test]
fn quitting_horizon_ends_no_session_of_a_machine() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    let _panel = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of the machine");

    app.begin_shutdown();
    app.run_exit_cleanup();
    assert!(
        app.session_kills.is_empty(),
        "the sessions of a machine outlive the window, which is the point of a place"
    );
}
