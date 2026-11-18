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
    )
    .with_key("build-1", "build-1");
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
        Some("tmux -u new-session -A -s hz-c8dfe3af-1 \\; set-option status off"),
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
        Some("tmux -u new-session -A -s hz-c8dfe3af-1 \\; set-option status off")
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

// The close of one panel of a machine asks first, in the words of the close of a
// whole workspace. These cases run frames of the window, so the press that a
// person makes is the press that the title bar and the ask answer.

use horizon_core::PanelId;

use crate::app::PANEL_TITLEBAR_HEIGHT;
use crate::app::test_support::{raw_input, run_app_frame_with_input};

/// One frame of the window, as eframe runs it, with `events` as its input.
fn frame(ctx: &egui::Context, app: &mut HorizonApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut input = raw_input([1400.0, 900.0], None);
    input.events = events;
    run_app_frame_with_input(ctx, app, input)
}

fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One press of `pos`: the frame that takes it down and the frame that lets it up.
fn press(ctx: &egui::Context, app: &mut HorizonApp, pos: egui::Pos2) -> egui::FullOutput {
    frame(ctx, app, vec![pointer(pos, true)]);
    frame(ctx, app, vec![pointer(pos, false)])
}

/// `frames` frames of the window with no input, and the output of the last one.
fn settle(ctx: &egui::Context, app: &mut HorizonApp, frames: usize) -> egui::FullOutput {
    let mut output = frame(ctx, app, Vec::new());
    for _ in 1..frames {
        output = frame(ctx, app, Vec::new());
    }
    output
}

/// The drawn texts of `output` with their centre.
fn texts(output: &egui::FullOutput) -> impl Iterator<Item = (&str, egui::Pos2)> {
    output.shapes.iter().filter_map(|shape| match &shape.shape {
        egui::Shape::Text(text) => Some((text.galley.text(), text.pos + text.galley.size() * 0.5)),
        _ => None,
    })
}

/// The line of the ask that the frame drew, when it drew one.
fn ask_line(output: &egui::FullOutput) -> Option<&str> {
    texts(output)
        .map(|(text, _)| text)
        .find(|text| text.starts_with("End the tmux session"))
}

/// The centre of the button `label` of the ask: the one nearest to its line,
/// because the line under the grid of a panel whose program ended says `Close` too.
fn ask_button(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    let line = texts(output)
        .find(|(text, _)| text.starts_with("End the tmux session"))
        .map(|(_, centre)| centre)
        .expect("the ask names the machine before it offers a button");
    texts(output)
        .filter(|(text, _)| *text == label)
        .map(|(_, centre)| centre)
        .min_by(|a, b| a.distance(line).total_cmp(&b.distance(line)))
        .unwrap_or_else(|| panic!("the ask offers {label}"))
}

/// The centre of the close control of the title bar of `panel`.
fn close_control(app: &HorizonApp, panel: PanelId) -> egui::Pos2 {
    let rect = app
        .panel_screen_rects
        .get(&panel)
        .copied()
        .expect("the panel is on screen");
    egui::Pos2::new(rect.max.x - 18.0, rect.min.y + PANEL_TITLEBAR_HEIGHT * 0.5)
}

/// A window that shows one panel of `place`, after the frames that settle it.
fn window_in(place: WorkspacePlace) -> (tempfile::TempDir, egui::Context, HorizonApp, PanelId) {
    let (temp, mut app, workspace) = app_in(place);
    let panel = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of the place");
    let ctx = egui::Context::default();
    for _ in 0..3 {
        frame(&ctx, &mut app, Vec::new());
    }
    (temp, ctx, app, panel)
}

#[test]
fn the_close_control_of_a_panel_of_a_machine_asks_before_the_session_ends() {
    let (_temp, ctx, mut app, panel) = window_in(WorkspacePlace::Machine("build-1".to_owned()));

    let close = close_control(&app, panel);
    press(&ctx, &mut app, close);
    // The close that a press of the control queues is applied on the next frame,
    // and the ask is drawn there.
    let output = settle(&ctx, &mut app, 2);
    assert!(
        app.board.panel(panel).is_some(),
        "the first press ends nothing: it asks"
    );
    assert_eq!(app.close_panel_confirm, Some(panel), "the press of the close asks");
    assert!(
        app.session_kills.is_empty(),
        "no tmux session ends before the person answers"
    );
    assert_eq!(
        ask_line(&output),
        Some("End the tmux session of this panel on build-1?"),
        "the ask uses the words of the close of a whole workspace"
    );

    let cancel = ask_button(&output, "Cancel");
    press(&ctx, &mut app, cancel);
    settle(&ctx, &mut app, 1);
    assert!(app.board.panel(panel).is_some(), "Cancel keeps the panel");
    assert_eq!(app.close_panel_confirm, None, "the ask is answered");
    assert!(
        app.panel_screen_rects.contains_key(&panel),
        "the panel that stays keeps its place on the canvas"
    );
    assert!(
        app.session_kills.is_empty(),
        "Cancel keeps the tmux session of the panel"
    );

    let close = close_control(&app, panel);
    press(&ctx, &mut app, close);
    let output = settle(&ctx, &mut app, 2);
    assert!(app.board.panel(panel).is_some(), "the next close asks again");
    let close = ask_button(&output, "Close");
    // The frame of the release runs the answered close, so the test reads the kill
    // before the next frame hands it to the worker that runs ssh.
    press(&ctx, &mut app, close);
    assert!(app.board.panel(panel).is_none(), "the red Close ends the panel");
    assert_eq!(
        app.session_kills
            .iter()
            .map(|kill| kill.session.as_str())
            .collect::<Vec<_>>(),
        ["hz-c8dfe3af-1"],
        "the answered close ends the one session that the panel held"
    );
    // The test reads the kill instead of letting the next frame hand it to the
    // worker that runs ssh, so the frame after the answer costs no connection.
    app.session_kills.clear();
    let output = settle(&ctx, &mut app, 1);
    assert_eq!(ask_line(&output), None, "the answered ask leaves no line on the canvas");
}

#[test]
fn a_panel_of_this_computer_closes_at_once_with_no_ask() {
    let (_temp, ctx, mut app, panel) = window_in(WorkspacePlace::Local);

    let close = close_control(&app, panel);
    press(&ctx, &mut app, close);
    let output = settle(&ctx, &mut app, 2);
    assert!(app.board.panel(panel).is_none(), "a local panel closes at once");
    assert_eq!(app.close_panel_confirm, None, "a local panel asks nothing");
    assert_eq!(ask_line(&output), None, "a local panel draws no ask");
    assert!(app.session_kills.is_empty(), "a local panel holds no session");
}

#[test]
fn the_notice_of_a_panel_of_a_machine_asks_before_the_session_ends() {
    let (_temp, mut app, workspace) = app_in(WorkspacePlace::Machine("build-1".to_owned()));
    let panel = app
        .create_panel_with_options(harmless(PanelOptions::default()), workspace)
        .expect("a terminal of the machine");

    // The line under the grid of a panel whose program ended queues its close, as
    // the close control of the title bar does, and the frame applies the queue.
    app.run_panel_notice_action(panel, crate::terminal_widget::PanelNoticeAction::ClosePanel);
    app.apply_panel_transitions();
    assert!(app.board.panel(panel).is_some(), "the notice asks before it ends work");
    assert_eq!(app.close_panel_confirm, Some(panel), "the notice of the close asks");
    assert!(app.session_kills.is_empty(), "the ask comes before the end");

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

#[test]
fn the_window_of_a_detached_workspace_draws_the_ask_of_its_own_close() {
    let (_temp, ctx, mut app, panel) = window_in(WorkspacePlace::Machine("build-1".to_owned()));
    let workspace = app.board.panel(panel).expect("panel").workspace_id;
    app.detach_workspace(workspace);
    app.request_close_panel(panel);
    assert_eq!(
        app.close_panel_confirm,
        Some(panel),
        "the close of a machine panel asks"
    );

    // The canvas of the root window draws no ask of a panel of a detached
    // workspace, so the window of that workspace owes the person the answer.
    let output = settle(&ctx, &mut app, 2);

    let asked = texts(&output)
        .filter(|(text, _)| text.starts_with("End the tmux session"))
        .count();
    assert_eq!(asked, 1, "the window that holds the panel draws its own ask once");
    assert_eq!(
        ask_line(&output),
        Some("End the tmux session of this panel on build-1?"),
        "the ask uses the words of the close of a whole workspace"
    );
    assert!(
        app.board.panel(panel).is_some(),
        "the ask ends no panel before the person answers"
    );
}
