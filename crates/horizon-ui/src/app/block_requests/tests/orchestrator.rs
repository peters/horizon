//! A card of the orchestrator of a workspace: the caller that its actor names,
//! the workspace that its card lands in, the operations that it reaches and the
//! one refusal of a caller without a panel, judged on a test board.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::agent_bridge::Entry;
use horizon_core::agent_bridge::actor;
use horizon_core::{MachineKeys, RuntimeState, StartupDecision};

type TestApp = (tempfile::TempDir, Context, HorizonApp);

/// The local ids of the two workspaces of the board. A workspace of this board
/// has a local id of the form that `new_local_id` makes, which is the only form
/// that an orchestrator actor carries.
const FIRST: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";
const SECOND: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f3a4b5c6d";

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

/// A request of the orchestrator of the workspace at `index`.
fn orchestrator_request(app: &HorizonApp, index: usize, operation: Operation) -> Request {
    Request {
        request_id: "test-orchestrator".into(),
        actor: actor::of_workspace(&workspace_local_id(app, index)).expect("the actor of a workspace"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The queue directory of a test, so no answer reaches the runtime root of the
/// person running the test.
fn queue(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("block-panel-requests");
    fs::create_dir_all(&root).expect("the queue directory");
    root
}

/// A request of the agent panel with `local_id`, which is the actor of a panel of
/// this board.
fn panel_request(local_id: &str, operation: Operation) -> Request {
    Request {
        request_id: "test-panel".into(),
        actor: format!("horizon:{local_id}"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The document of a card that asks for one press.
fn document(heading: &str) -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": heading},
            {"type": "buttons", "items": [{"id": "go", "label": "Go", "tone": "primary"}]}
        ]
    })
}

/// The card that one create answers, in the workspace at `index`.
fn created_in(app: &mut HorizonApp, ctx: &Context, index: usize, heading: &str) -> String {
    let outcome = app.apply_block_request(
        &orchestrator_request(
            app,
            index,
            Operation::Create {
                title: Some(heading.into()),
                document: document(heading),
            },
        ),
        ctx,
    );
    match outcome {
        Outcome::Created { panel_id, revision } => {
            assert_eq!(revision, 1);
            panel_id
        }
        other => panic!("expected the card of the orchestrator: {other:?}"),
    }
}

/// Records one press on the button `component_id` of the card `card`, as the
/// widget does when the person clicks it.
fn press(app: &mut HorizonApp, card: &str, component_id: &str) {
    let id = app.board.panel_id_by_local_id(card).expect("the card");
    let revision = app.board.panel(id).and_then(Panel::block).expect("the card").revision;
    let document = app
        .board
        .panel(id)
        .and_then(Panel::block)
        .expect("the card")
        .document
        .clone();
    let mut runtime = app.panel_render_caches.card_runtime.remove(&id).unwrap_or_default();
    assert!(
        runtime.press(component_id, revision, &document, manifest::now_millis()),
        "the card queues the press"
    );
    app.panel_render_caches.card_runtime.insert(id, runtime);
}

/// The code of one refusal.
fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn every_request_of_an_orchestrator_names_the_orchestrator_of_its_workspace() {
    let (temp, ctx, mut app) = app();
    present(&mut app, &ctx, temp.path(), 1);
    let local_id = workspace_local_id(&app, 1);
    let workspace = app.board.workspaces[1].id;
    let request = orchestrator_request(&app, 1, Operation::List);
    assert_eq!(
        request.actor,
        format!("horizon:orchestrator/{local_id}"),
        "the actor of a workspace holds its local id"
    );
    let Ok(CardCaller::Orchestrator { workspace: named }) = app.request_caller(&request) else {
        panic!("the host accepts the orchestrator of a workspace of its board");
    };
    assert_eq!(named, workspace);
    assert!(
        app.owner_workspace(&request.actor)
            .is_some_and(|owner| owner == workspace),
        "the workspace of the cards of that actor is the workspace of its local id"
    );
}

#[test]
fn an_orchestrator_actor_of_an_unknown_workspace_is_caller_unavailable() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    let mut request = orchestrator_request(
        &app,
        0,
        Operation::Create {
            title: Some("Plan".into()),
            document: document("Plan"),
        },
    );
    request.actor = actor::of_workspace("11111111-2222-4333-8444-555555555555").expect("a local id");
    let outcome = app.apply_block_request(&request, &ctx);
    assert_eq!(code(&outcome), "caller_unavailable", "{outcome:?}");
    assert_eq!(app.board.panels.len(), before, "a refused request draws no card");
}

#[test]
fn a_card_of_an_orchestrator_lands_in_the_workspace_of_its_actor() {
    let (temp, ctx, mut app) = app();
    present(&mut app, &ctx, temp.path(), 0);
    let active = app.board.active_workspace.expect("an active workspace");
    let first = app.board.workspaces[0].id;
    let second = app.board.workspaces[1].id;
    assert_eq!(active, second, "the second workspace is the active one of the board");

    let card = created_in(&mut app, &ctx, 0, "Plan");

    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    let panel = app.board.panel(id).expect("the card");
    assert_eq!(
        panel.workspace_id, first,
        "the card lands in the workspace that the actor names, not in the active one"
    );
    let block = panel.block().expect("a card");
    assert_eq!(
        block.owner_actor.as_deref(),
        Some(
            actor::of_workspace(&workspace_local_id(&app, 0))
                .expect("the actor")
                .as_str()
        )
    );
    assert_eq!(
        block.place,
        CardPlace::Tile,
        "a caller without a panel has no column of a panel: its card takes a free tile"
    );
    assert!(
        app.workspace_cards(second).is_empty(),
        "no card of this orchestrator stands in the active workspace"
    );
    assert_eq!(
        app.workspace_cards(first).len(),
        1,
        "the card of the orchestrator is the one card of its workspace"
    );
}

#[test]
fn every_card_operation_of_an_orchestrator_works() {
    let (temp, ctx, mut app) = app();
    present(&mut app, &ctx, temp.path(), 1);
    let card = created_in(&mut app, &ctx, 1, "Plan");

    // update
    let updated = app.apply_block_request(
        &orchestrator_request(
            &app,
            1,
            Operation::Update {
                panel_id: card.clone(),
                document: document("Plan, second revision"),
            },
        ),
        &ctx,
    );
    assert!(
        matches!(updated, Outcome::Updated { revision: 2, .. }),
        "the owner changes its own card: {updated:?}"
    );

    // list
    let listed = app.apply_block_request(&orchestrator_request(&app, 1, Operation::List), &ctx);
    let Outcome::Panels { panels } = &listed else {
        panic!("expected the card list: {listed:?}");
    };
    assert_eq!(panels.len(), 1);
    assert_eq!(panels[0].panel_id, card);
    assert!(panels[0].owned_by_caller, "the orchestrator owns the card");
    // The orchestrator of the other workspace runs too, so its read is accepted
    // and answers the cards of its own workspace alone.
    present(&mut app, &ctx, temp.path(), 0);
    let other = app.apply_block_request(&orchestrator_request(&app, 0, Operation::List), &ctx);
    let Outcome::Panels { panels } = &other else {
        panic!("expected the card list: {other:?}");
    };
    assert!(
        panels.is_empty(),
        "the orchestrator of another workspace reads no card of this one"
    );

    every_card_read_of_an_orchestrator(&mut app, &ctx, &card);
}

/// The reads and the changes of one card of an orchestrator that remains on the
/// board: `inspect`, `visibility` and the card of the list.
fn every_card_read_of_an_orchestrator(app: &mut HorizonApp, ctx: &Context, card: &str) {
    // inspect
    let inspected = app.apply_block_request(
        &orchestrator_request(
            app,
            1,
            Operation::Inspect {
                panel_id: card.to_owned(),
            },
        ),
        ctx,
    );
    let Outcome::Inspected {
        owned_by_caller,
        agent_present,
        ..
    } = &inspected
    else {
        panic!("expected the card: {inspected:?}");
    };
    assert!(owned_by_caller);
    assert!(
        *agent_present,
        "the program of the dock runs for this workspace, so its orchestrator is present"
    );

    // next_click, with the click that a person made
    press(app, card, "go");
    let clicked = app.apply_block_request(
        &orchestrator_request(
            app,
            1,
            Operation::NextClick {
                panel_id: None,
                timeout_millis: Some(0),
            },
        ),
        ctx,
    );
    let Outcome::Clicked { click } = &clicked else {
        panic!("the wait of the orchestrator reads its click: {clicked:?}");
    };
    assert_eq!(click.component_id, "go");
    assert_eq!(click.panel_id, card);

    // visibility
    let hidden = app.apply_block_request(
        &orchestrator_request(
            app,
            1,
            Operation::Visibility {
                panel_id: card.to_owned(),
                visible: false,
            },
        ),
        ctx,
    );
    assert!(
        matches!(hidden, Outcome::Visibility { visible: false, .. }),
        "the owner hides its own card: {hidden:?}"
    );

    // close
    let closed = app.apply_block_request(
        &orchestrator_request(
            app,
            1,
            Operation::Close {
                panel_id: card.to_owned(),
            },
        ),
        ctx,
    );
    let Outcome::Closed {
        by_owner,
        closed_by: by,
        ..
    } = &closed
    else {
        panic!("the owner closes its own card: {closed:?}");
    };
    assert!(by_owner);
    assert!(by.is_empty(), "the caller has no panel of this board to name");
    assert!(
        app.board.panel_id_by_local_id(card).is_none(),
        "the card left the board"
    );
}

#[test]
fn a_next_click_that_names_the_card_of_an_orchestrator_waits_for_it() {
    let (temp, ctx, mut app) = app();
    present(&mut app, &ctx, temp.path(), 1);
    let card = created_in(&mut app, &ctx, 1, "Plan");
    let wait = app.apply_block_request_waiting(
        &orchestrator_request(
            &app,
            1,
            Operation::NextClick {
                panel_id: Some(card.clone()),
                timeout_millis: Some(30_000),
            },
        ),
        &ctx,
    );
    assert!(
        wait.is_none(),
        "the host holds the wait open for the press of the person: {wait:?}"
    );
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    assert!(app.block_listening(id), "the card reports that its agent listens");
    press(&mut app, &card, "go");
    assert!(
        app.settle_block_clicks(),
        "the press of the person answers the open wait of the orchestrator"
    );
}

#[test]
fn the_machine_operations_of_an_orchestrator_reach_the_machine_list() {
    let (temp, ctx, mut app) = app();
    present(&mut app, &ctx, temp.path(), 1);
    let root = queue(&temp);
    let key_directory = MachineKeys::from_home(app.session_store.home()).key_directory();
    fs::create_dir_all(&key_directory).expect("the key directory");
    fs::write(key_directory.join("build-1"), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the private key");
    fs::write(
        key_directory.join("build-1.pub"),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI horizon-machine-build-1\n",
    )
    .expect("the public key");

    let set = Operation::MachineSet {
        name: "build-1".into(),
        ssh: TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        },
        key: Some("build-1".into()),
        session: Some("work".into()),
        provider: Some("Hetzner, hel1".into()),
        repository: None,
        folder: None,
        size: None,
        place: None,
        price_per_hour: None,
        cards: false,
    };
    let answer = answered(&mut app, &ctx, &root, 1, set);
    assert!(
        matches!(&answer, Outcome::MachineSet { name, created } if name == "build-1" && *created),
        "the orchestrator registers a machine: {answer:?}"
    );

    let listed = answered(&mut app, &ctx, &root, 1, Operation::Machines);
    let Outcome::Machines { machines, .. } = &listed else {
        panic!("expected the machine list: {listed:?}");
    };
    assert_eq!(machines.len(), 1);
    assert_eq!(machines[0].name, "build-1");

    let key = answered(
        &mut app,
        &ctx,
        &root,
        1,
        Operation::MachineKey { name: "build-1".into() },
    );
    let Outcome::MachineKey { name, public_key, .. } = &key else {
        panic!("expected the key of the machine: {key:?}");
    };
    assert_eq!(name, "build-1");
    assert!(public_key.starts_with("ssh-ed25519 "));
}

/// One machine operation of the orchestrator of the workspace at `index`, whose
/// answer the coordination worker writes into `root`.
fn answered(app: &mut HorizonApp, ctx: &Context, root: &Path, index: usize, operation: Operation) -> Outcome {
    let request = orchestrator_request(app, index, operation);
    assert!(
        app.apply_block_request_in(&request, root, ctx).is_none(),
        "a machine operation answers from the coordination worker"
    );
    app.settle_browser_host_io();
    horizon_browser_control::manifest::block::take_result_at(root, &request)
        .expect("the answer is readable")
        .expect("the worker answered")
}

#[test]
fn an_operation_that_needs_the_panel_of_the_caller_is_refused() {
    let (temp, ctx, mut app) = app();
    present(&mut app, &ctx, temp.path(), 1);
    let root = queue(&temp);
    let terminal = app.apply_block_request(
        &orchestrator_request(
            &app,
            1,
            Operation::OpenTerminal {
                ssh: Some(TerminalRoute {
                    host: "203.0.113.10".into(),
                    user: None,
                    port: None,
                }),
                remote_command: None,
                title: None,
                key: None,
                machine: None,
            },
        ),
        &ctx,
    );
    assert_eq!(code(&terminal), "caller_unavailable", "{terminal:?}");
    let Outcome::Failed { message, .. } = &terminal else {
        unreachable!("a refusal names its reason");
    };
    assert_eq!(message, "The agent that asked is no longer in this host");

    // A panel of a preset is opened beside the caller panel, so a caller with no
    // panel is refused there too, with the same code.
    let request = orchestrator_request(
        &app,
        1,
        Operation::OpenPanel {
            preset: "Shell".into(),
            directory: std::env::temp_dir().display().to_string(),
            title: None,
            prompt: None,
        },
    );
    assert!(
        app.apply_block_request_in(&request, &root, &ctx).is_none(),
        "the read of the folder answers on the coordination worker"
    );
    app.settle_browser_host_io();
    let panel = horizon_browser_control::manifest::block::take_result_at(&root, &request)
        .expect("the answer is readable")
        .expect("the worker answered");
    assert_eq!(code(&panel), "caller_unavailable", "{panel:?}");
}

/// The stand-in program of the dock: it names its session and then reads the
/// lines of the person until the bridge stops, so a test holds a running bridge
/// without an agent program.
const SILENT: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
while IFS= read -r line; do :; done
"#;

/// The stand-in program of a conversation whose program ends at once, so a test
/// reads an ended bridge.
const EXITING: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"00000000-0000-4000-8000-000000000001"}'
exit 0
"#;

/// Starts the program of the dock for the workspace at `index` with the stand-in
/// `body`, and answers the local id of that workspace. The stricter caller rule
/// accepts the actor of an orchestrator only while this bridge runs.
fn present_with(app: &mut HorizonApp, ctx: &Context, folder: &Path, index: usize, body: &str) -> String {
    let program = horizon_core::stand_in_program_in(folder, "stand-in.sh", body).expect("the stand-in program");
    app.orchestrator_dock.set_program(program);
    let local_id = workspace_local_id(app, index);
    assert!(
        app.say_to_orchestrator(ctx, &local_id, "hello"),
        "the message of the person reaches the conversation of the workspace"
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !app.orchestrator_dock.running(&local_id) {
        assert!(std::time::Instant::now() < deadline, "the program of the dock starts");
        app.poll_orchestrator_dock(&egui::Context::default());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    local_id
}

/// [`present_with`] with the stand-in that reads the lines of the person.
fn present(app: &mut HorizonApp, ctx: &Context, folder: &Path, index: usize) -> String {
    present_with(app, ctx, folder, index, SILENT)
}

/// Waits until the bridge of the workspace `local_id` ended, which the program of
/// its conversation does by itself.
fn wait_until_ended(app: &mut HorizonApp, local_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.orchestrator_dock.running(local_id) {
        assert!(std::time::Instant::now() < deadline, "the program of the dock ends");
        app.poll_orchestrator_dock(&egui::Context::default());
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_card_that_the_person_moved_to_another_workspace_has_no_present_owner() {
    let (temp, ctx, mut app) = app();
    let second = app.board.workspaces[1].id;
    let owner_panel = app.board.panels[0].local_id.clone();
    let closing_panel = app.board.panels[1].local_id.clone();

    // The card of the orchestrator of the first workspace of this board.
    let local_id = present(&mut app, &ctx, temp.path(), 0);
    assert_eq!(local_id, workspace_local_id(&app, 0));
    let card = created_in(&mut app, &ctx, 0, "Plan");
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    assert!(
        app.card_agent_present(id),
        "the program of the dock runs, so the card of its orchestrator is live"
    );
    // The two panels of this board are agent panels, so an actor of either of
    // them reaches the card operations.
    for panel in &mut app.board.panels {
        if panel.local_id == owner_panel || panel.local_id == closing_panel {
            panel.kind = PanelKind::Claude;
        }
    }

    // The person moves the card to the other workspace.
    app.board.assign_panel_to_workspace(id, second);
    assert!(
        !app.card_agent_present(id),
        "the orchestrator is not in the workspace that the card moved to"
    );
    let chip = app.card_titlebar_chip(id).expect("the card has a state chip");
    assert_eq!(
        chip.state,
        crate::block_widget::chip::ChipState::Gone,
        "the chip of the moved card says `No agent`"
    );

    // A card of a panel owner that the person moved in the same way behaves the
    // same way: its owner is the panel, and that panel is in another workspace.
    let create = panel_request(
        &owner_panel,
        Operation::Create {
            title: Some("Notes".into()),
            document: document("Notes"),
        },
    );
    let Outcome::Created { panel_id, .. } = app.apply_block_request(&create, &ctx) else {
        panic!("the agent panel draws a card");
    };
    let panel_card = app.board.panel_id_by_local_id(&panel_id).expect("the card");
    assert!(
        app.card_agent_present(panel_card),
        "the owner panel is in the workspace of its card"
    );
    app.board.assign_panel_to_workspace(panel_card, second);
    assert!(
        !app.card_agent_present(panel_card),
        "a card of a panel owner that left its workspace has no present owner either"
    );

    // An agent of the workspace that holds a moved card closes it, as it closes
    // the card of a panel that is not in that workspace.
    let closed = app.apply_block_request(
        &panel_request(&closing_panel, Operation::Close { panel_id: card.clone() }),
        &ctx,
    );
    let Outcome::Closed { by_owner, .. } = &closed else {
        panic!("the agent of the workspace closes the card of the moved orchestrator: {closed:?}");
    };
    assert!(!by_owner, "the closer is not the owner of the card");
    assert!(
        app.board.panel_id_by_local_id(&card).is_none(),
        "the card left the board"
    );
}

#[test]
fn the_buttons_of_a_card_of_an_orchestrator_are_dark_without_its_bridge() {
    let (temp, ctx, mut app) = app();
    // The card of an orchestrator is drawn while its program runs, so the stop of
    // that program is the state that this test reads.
    let local_id = present(&mut app, &ctx, temp.path(), 1);
    let card = created_in(&mut app, &ctx, 1, "Plan");
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    assert!(app.card_agent_present(id), "the program of the dock runs");
    app.stop_orchestrator_conversation(&local_id);
    assert!(
        !app.card_agent_present(id),
        "the stopped conversation holds no bridge, so the card has no present owner"
    );
    let chip = app.card_titlebar_chip(id).expect("the card has a state chip");
    assert_eq!(
        chip.state,
        crate::block_widget::chip::ChipState::Gone,
        "the chip of a card without its orchestrator says `No agent`"
    );
    // A press that no wait of the orchestrator reads stays in the queue of the
    // card: the stopped program takes no wake, so this frame sends nothing.
    press(&mut app, &card, "go");
    assert!(
        !app.settle_block_clicks(),
        "no wake line and no answer leaves this frame for a caller without a bridge"
    );
    assert_eq!(
        app.card_pending_clicks(id),
        1,
        "the press waits in the queue of the card for the next wait of its owner"
    );
}

#[test]
fn an_orchestrator_is_a_caller_only_while_its_bridge_runs() {
    let (temp, ctx, mut app) = app();
    let request = orchestrator_request(&app, 0, Operation::List);

    // No bridge runs for this workspace, so the actor of its orchestrator names no
    // caller, although the workspace is on the board.
    assert_eq!(
        code(&app.apply_block_request(&request, &ctx)),
        "caller_unavailable",
        "an identity of a program that Horizon did not start is no caller"
    );
    assert!(
        app.owner_workspace(&request.actor).is_none(),
        "and its cards are unreadable"
    );

    let local_id = present(&mut app, &ctx, temp.path(), 0);
    assert!(
        matches!(app.request_caller(&request), Ok(CardCaller::Orchestrator { .. })),
        "the running program of the dock is the caller of its workspace"
    );
    assert_eq!(app.owner_workspace(&request.actor), Some(app.board.workspaces[0].id));

    // A program that ends takes the identity with it.
    app.stop_orchestrator_conversation(&local_id);
    assert!(
        !app.orchestrator_dock.running(&local_id),
        "a stopped conversation owns no bridge"
    );
    assert_eq!(
        code(&app.apply_block_request(&request, &ctx)),
        "caller_unavailable",
        "the identity ends with the bridge of the program"
    );
}

#[test]
fn an_ended_bridge_is_no_caller_either() {
    let (temp, ctx, mut app) = app();
    let local_id = present_with(&mut app, &ctx, temp.path(), 0, EXITING);
    wait_until_ended(&mut app, &local_id);
    let request = orchestrator_request(&app, 0, Operation::List);

    assert_eq!(
        code(&app.apply_block_request(&request, &ctx)),
        "caller_unavailable",
        "a program that ended leaves no caller behind"
    );
    assert!(app.owner_workspace(&request.actor).is_none());
}

#[test]
fn the_conversation_of_the_workspace_names_the_card_of_its_orchestrator() {
    let (temp, ctx, mut app) = app();
    let local_id = present(&mut app, &ctx, temp.path(), 0);
    let card = created_in(&mut app, &ctx, 0, "Plan");

    let rows = |app: &HorizonApp| -> Vec<(String, String)> {
        app.orchestrator_dock
            .conversation_entries(&local_id)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Card { panel, title } => Some((panel, title)),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        rows(&app),
        vec![(card.clone(), "Plan".to_owned())],
        "the create adds one reference row of the card"
    );

    // A second create of the same card, and an update of its heading, keep one row.
    let again = created_in(&mut app, &ctx, 0, "Plan");
    assert_ne!(again, card, "a second create draws a second card");
    let updated = created_in(&mut app, &ctx, 0, "Plan");
    app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::Update {
                panel_id: updated.clone(),
                document: document("Plan, second revision"),
            },
        ),
        &ctx,
    );
    let rows_now = rows(&app);
    assert_eq!(
        rows_now.len(),
        3,
        "one row for each card of the conversation: {rows_now:?}"
    );
    assert!(
        rows_now.contains(&(updated.clone(), "Plan, second revision".to_owned())),
        "the row of a card follows the heading of its document: {rows_now:?}"
    );
    assert_eq!(
        rows_now.iter().filter(|(panel, _)| *panel == updated).count(),
        1,
        "an update adds no second row"
    );

    // A card that leaves the board keeps its row: the dock draws it as a closed
    // row, which is what a restart reads as well.
    app.apply_block_request(
        &orchestrator_request(&app, 0, Operation::Close { panel_id: card.clone() }),
        &ctx,
    );
    assert!(
        rows(&app).iter().any(|(panel, _)| *panel == card),
        "the row of a closed card stays, so the person reads that it closed"
    );
    assert!(
        app.board.panel_id_by_local_id(&card).is_none(),
        "the card left the board"
    );
}

/// The front desk of the dock owns no workspace, so every operation of that actor
/// is refused: it creates no card, reads no card and opens no panel. The caller is
/// named by the one constant of the bridge, and no workspace of the board can
/// match it.
#[test]
fn the_front_desk_actor_is_caller_unavailable_for_every_operation() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    let operations = [
        Operation::List,
        Operation::Create {
            title: Some("Plan".into()),
            document: document("Plan"),
        },
        Operation::Update {
            panel_id: "any-card".into(),
            document: document("Plan"),
        },
        Operation::Inspect {
            panel_id: "any-card".into(),
        },
        Operation::Visibility {
            panel_id: "any-card".into(),
            visible: false,
        },
        Operation::Close {
            panel_id: "any-card".into(),
        },
    ];
    for operation in operations {
        let mut request = orchestrator_request(&app, 0, operation.clone());
        request.actor = actor::FRONT_DESK.to_owned();
        let outcome = app.apply_block_request(&request, &ctx);
        assert_eq!(
            code(&outcome),
            "caller_unavailable",
            "the front desk asks for {operation:?}: {outcome:?}"
        );
    }
    assert_eq!(app.board.panels.len(), before, "a refused request draws no card");
    assert!(
        app.owner_workspace(actor::FRONT_DESK).is_none(),
        "the front desk owns no workspace, so it reads no card"
    );
    let mut caller = orchestrator_request(&app, 0, Operation::List);
    caller.actor = actor::FRONT_DESK.to_owned();
    assert!(
        app.request_caller(&caller).is_err(),
        "the host refuses the actor of the front desk before any operation runs"
    );
}

/// A card is the answer of the orchestrator that the person writes to, so it
/// comes into the view of the canvas. A card of a conversation that the dock does
/// not show moves no view, and no card takes the keyboard.
#[test]
fn a_new_card_of_the_shown_conversation_comes_into_view_and_no_other_card_moves_the_view() {
    let (temp, ctx, mut app) = app();
    let _local_id = present(&mut app, &ctx, temp.path(), 0);
    let first = app.board.workspaces[0].id;
    app.board.focus_workspace(first);
    let focused = app.board.focused;
    // The view stands far from every panel of the board.
    app.canvas_view.set_pan_offset([-9000.0, -9000.0]);
    let far = app.canvas_view.pan_offset;

    // The dock shows the front desk: the person does not look at this
    // conversation, so its card moves nothing.
    app.orchestrator_dock.front_desk_mut().show();
    let _ = created_in(&mut app, &ctx, 0, "Plan");
    assert_eq!(app.canvas_view.pan_offset, far, "an unseen conversation moves no view");

    // The dock shows the conversation of the workspace: its card comes into view.
    app.orchestrator_dock.front_desk_mut().show_workspace();
    let card = created_in(&mut app, &ctx, 0, "Answer");
    assert_ne!(
        app.canvas_view.pan_offset, far,
        "the card of the conversation that the dock shows comes into view"
    );
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    assert_ne!(app.board.focused, Some(id), "the card takes no key of the person");
    assert_eq!(app.board.focused, focused, "the focus of the board stays where it was");
}
