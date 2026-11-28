//! The terminal of a machine that the orchestrator of a workspace opens.
//!
//! The orchestrator of the dock has no panel of its own, so the terminal of a
//! machine is the one panel that it opens. These tests read its workspace, the
//! mark of its titlebar, the actor that it records, the bound of terminals that
//! it shares with a panel, and the two operations that it may not ask for.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::app::block_machines::settle_machine_load;
use crate::app::machine_open::ORCHESTRATOR_MARK;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::agent_bridge::actor;
use horizon_core::browser::manifest::block::MAX_TERMINALS_PER_OWNER;
use horizon_core::{MachineKeys, PanelKind, RuntimeState, StartupDecision};

/// The local ids of the two workspaces of the board.
const FIRST: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";
const SECOND: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f3a4b5c6d";

/// The machine that the record of these tests names.
const MACHINE: &str = "cloud-1";

/// The words of the two refusals that these tests read.
const NO_ROUTE_OF_ITS_OWN: &str = "An orchestrator opens the terminal of a machine of the machine list: pass machine. A route of its own needs an agent panel.";
const NO_PANEL_TO_STAND_BESIDE: &str =
    "An orchestrator has no panel to stand beside: start an agent program with worker_start.";

type TestApp = (tempfile::TempDir, Context, HorizonApp);

/// The board of these tests: two workspaces, each with one agent panel.
fn app() -> TestApp {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state(FIRST, [0.0, 0.0]),
            editor_workspace_state(SECOND, [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    // The startup read of the machine list answers before a test writes one.
    settle_machine_load(&mut app);
    (temp, ctx, app)
}

/// The local id of the workspace at `index` of the board.
fn workspace_local_id(app: &HorizonApp, index: usize) -> String {
    app.board
        .workspaces
        .get(index)
        .map(|workspace| workspace.local_id.clone())
        .expect("the workspace of the test board")
}

/// Says that the dock runs the program of the workspace at `index`, so its actor
/// is a caller of this host without an agent program behind it.
fn running(app: &mut HorizonApp, index: usize) -> String {
    let local_id = workspace_local_id(app, index);
    app.orchestrator_dock.mark_running_for_test(&local_id);
    local_id
}

/// A request of the orchestrator of the workspace at `index`.
fn orchestrator_request(app: &HorizonApp, index: usize, operation: Operation) -> Request {
    Request {
        request_id: "test-orchestrator-terminal".into(),
        actor: actor::of_workspace(&workspace_local_id(app, index)).expect("the actor of a workspace"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// A request of the agent panel at `index` of the board.
fn panel_request(app: &HorizonApp, index: usize, operation: Operation) -> Request {
    Request {
        request_id: "test-panel-terminal".into(),
        actor: format!("horizon:{}", app.board.panels[index].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// A request of the front desk.
fn front_desk_request(operation: Operation) -> Request {
    Request {
        request_id: "test-front-desk-terminal".into(),
        actor: actor::FRONT_DESK.into(),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The queue of the block requests of this test.
fn queue(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("block-panel-requests");
    fs::create_dir_all(&root).expect("the queue directory");
    root
}

/// Writes the key pair of `name` into the home of this test.
fn plant_key(app: &HorizonApp, name: &str) {
    let directory = MachineKeys::from_home(app.session_store.home()).key_directory();
    fs::create_dir_all(&directory).expect("the key directory");
    fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the private key");
    fs::write(directory.join(format!("{name}.pub")), "ssh-ed25519 AAAA\n").expect("the public key");
}

/// One machine record of the list, with one session and one key.
fn set_machine(name: &str) -> Operation {
    Operation::MachineSet {
        name: name.into(),
        ssh: TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        },
        key: Some(name.into()),
        session: Some("work".into()),
        provider: None,
        repository: None,
        folder: None,
        size: None,
        place: None,
        price_per_hour: None,
        cards: false,
    }
}

/// One `open_terminal` of the machine of the record.
fn open_machine() -> Operation {
    Operation::OpenTerminal {
        ssh: None,
        remote_command: None,
        title: None,
        key: None,
        machine: Some(MACHINE.into()),
    }
}

/// One `open_terminal` with a route of its own.
fn open_route() -> Operation {
    Operation::OpenTerminal {
        ssh: Some(TerminalRoute {
            host: "203.0.113.99".into(),
            user: None,
            port: None,
        }),
        remote_command: None,
        title: None,
        key: None,
        machine: None,
    }
}

/// The code of one refusal.
fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// The words of one refusal.
fn message(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { message, .. } => message,
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// One machine operation of the orchestrator of the workspace at `index`, whose
/// answer the coordination worker writes into `root`.
fn answered(app: &mut HorizonApp, ctx: &Context, root: &Path, index: usize, operation: Operation) -> Outcome {
    let request = orchestrator_request(app, index, operation);
    assert!(
        app.apply_block_request_in(&request, root, ctx).is_none(),
        "the request answers from the coordination worker"
    );
    app.settle_browser_host_io();
    block::take_result_at(root, &request)
        .expect("the answer is readable")
        .expect("the worker answered")
}

/// The board of one test, with the machine record of `MACHINE` in its list.
fn app_with_machine() -> (TestApp, PathBuf) {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, MACHINE);
    let request = panel_request(&app, 0, set_machine(MACHINE));
    assert!(
        app.apply_block_request_in(&request, &root, &ctx).is_none(),
        "the record answers from the coordination worker"
    );
    app.settle_browser_host_io();
    block::take_result_at(&root, &request)
        .expect("the answer is readable")
        .expect("the worker answered");
    ((temp, ctx, app), root)
}

#[test]
fn an_orchestrator_opens_the_terminal_of_a_machine_of_the_list_in_its_workspace() {
    let ((_temp, ctx, mut app), root) = app_with_machine();
    running(&mut app, 0);
    let workspace = app.board.workspaces[0].id;
    let keyboard = app.board.panels[1].id;
    app.board.focus(keyboard);
    let before = app.board.panels.len();

    let answer = answered(&mut app, &ctx, &root, 0, open_machine());

    let Outcome::Opened { panel_id } = &answer else {
        panic!("expected an opened terminal: {answer:?}");
    };
    assert_eq!(app.board.panels.len(), before + 1, "one terminal opens");
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(panel_id).expect("the terminal"))
        .expect("the terminal");
    assert_eq!(panel.workspace_id, workspace, "the terminal lands in its workspace");
    assert_eq!(panel.kind, PanelKind::Ssh, "the terminal is an SSH panel");
    assert_eq!(panel.machine(), Some(MACHINE), "the panel is tied to the machine");
    assert_eq!(panel.opened_by(), Some(ORCHESTRATOR_MARK));
    assert_eq!(
        panel.opened_by_actor(),
        Some(
            actor::of_workspace(&workspace_local_id(&app, 0))
                .expect("the actor of a workspace")
                .as_str()
        ),
        "the terminal records the actor of the request of the orchestrator"
    );
    assert_eq!(app.board.focused, Some(keyboard), "the keyboard stays where it was");
    let args = panel.ssh_connection.as_ref().expect("the connection").to_command_args();
    assert!(
        args.contains(&"root@203.0.113.10".to_string()),
        "the route comes from the record: {args:?}"
    );
    assert!(
        args.contains(&"tmux -u new-session -A -s work \\; set-option status off".to_string()),
        "the remote command comes from the record: {args:?}"
    );
}

#[test]
fn a_route_of_its_own_is_refused_with_the_words_of_a_machine() {
    let ((_temp, ctx, mut app), _root) = app_with_machine();
    running(&mut app, 0);
    let before = app.board.panels.len();

    let refused = app.apply_block_request(&orchestrator_request(&app, 0, open_route()), &ctx);

    assert_eq!(code(&refused), "not_allowed", "{refused:?}");
    assert_eq!(message(&refused), NO_ROUTE_OF_ITS_OWN);
    assert_eq!(app.board.panels.len(), before, "a route of its own opens no panel");
}

#[test]
fn the_panel_of_a_preset_is_refused_with_the_way_to_a_panel_of_its_own() {
    let ((_temp, ctx, mut app), root) = app_with_machine();
    running(&mut app, 0);
    let before = app.board.panels.len();

    let refused = answered(
        &mut app,
        &ctx,
        &root,
        0,
        Operation::OpenPanel {
            preset: "Shell".into(),
            directory: std::env::temp_dir().display().to_string(),
            title: None,
            prompt: None,
        },
    );

    assert_eq!(code(&refused), "caller_unavailable", "{refused:?}");
    assert_eq!(message(&refused), NO_PANEL_TO_STAND_BESIDE);
    assert_eq!(app.board.panels.len(), before, "a refused preset opens no panel");
}

#[test]
fn the_front_desk_is_still_no_caller_of_the_terminal_of_a_machine() {
    let ((_temp, ctx, mut app), _root) = app_with_machine();
    app.orchestrator_dock
        .mark_running_for_test(crate::app::orchestrator_dock::FRONT_DESK);
    let before = app.board.panels.len();

    let refused = app.apply_block_request(&front_desk_request(open_machine()), &ctx);

    assert_eq!(code(&refused), "caller_unavailable", "{refused:?}");
    assert_eq!(app.board.panels.len(), before, "the front desk opens no panel");
}

#[test]
fn the_bound_of_terminals_holds_for_the_actor_of_the_orchestrator() {
    let ((_temp, ctx, mut app), root) = app_with_machine();
    let local_id = running(&mut app, 0);
    let before = app.board.panels.len();

    for _ in 0..MAX_TERMINALS_PER_OWNER {
        let answer = answered(&mut app, &ctx, &root, 0, open_machine());
        assert!(
            matches!(answer, Outcome::Opened { .. }),
            "the orchestrator opens each terminal under the bound: {answer:?}"
        );
    }
    let ninth = answered(&mut app, &ctx, &root, 0, open_machine());

    assert_eq!(code(&ninth), "too_many_terminals", "{ninth:?}");
    assert_eq!(
        app.board.panels.len(),
        before + MAX_TERMINALS_PER_OWNER,
        "the ninth terminal opens nothing"
    );
    // The bound belongs to the actor of the orchestrator, not to its workspace:
    // a panel of that workspace keeps its own allowance.
    let request = panel_request(&app, 0, open_machine());
    assert!(
        app.apply_block_request_in(&request, &root, &ctx).is_none(),
        "the request answers from the coordination worker"
    );
    app.settle_browser_host_io();
    let from_panel = block::take_result_at(&root, &request)
        .expect("the answer is readable")
        .expect("the worker answered");
    assert!(
        matches!(from_panel, Outcome::Opened { .. }),
        "a panel of the workspace opens its own terminal: {from_panel:?}"
    );
    assert_eq!(
        app.agent_opens
            .terminals
            .count(&actor::of_workspace(&local_id).expect("the actor of a workspace")),
        MAX_TERMINALS_PER_OWNER,
        "the terminal of a panel counts for the panel, not for the orchestrator"
    );
}
