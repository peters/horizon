//! A card of the orchestrator of a workspace: the caller that its actor names,
//! the workspace that its card lands in, the operations that it reaches and the
//! one refusal of a caller without a panel, judged on a test board.

use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
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
    let (_temp, _ctx, app) = app();
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
    let (_temp, ctx, mut app) = app();
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
    let (_temp, ctx, mut app) = app();
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
    let other = app.apply_block_request(&orchestrator_request(&app, 0, Operation::List), &ctx);
    let Outcome::Panels { panels } = &other else {
        panic!("expected the card list: {other:?}");
    };
    assert!(
        panels.is_empty(),
        "the orchestrator of another workspace reads no card of this one"
    );

    // inspect
    let inspected = app.apply_block_request(
        &orchestrator_request(&app, 1, Operation::Inspect { panel_id: card.clone() }),
        &ctx,
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
        !agent_present,
        "no bridge runs for this workspace in a test, so its orchestrator is not present"
    );

    // next_click, with the click that a person made
    press(&mut app, &card, "go");
    let clicked = app.apply_block_request(
        &orchestrator_request(
            &app,
            1,
            Operation::NextClick {
                panel_id: None,
                timeout_millis: Some(0),
            },
        ),
        &ctx,
    );
    let Outcome::Clicked { click } = &clicked else {
        panic!("the wait of the orchestrator reads its click: {clicked:?}");
    };
    assert_eq!(click.component_id, "go");
    assert_eq!(click.panel_id, card);

    // visibility
    let hidden = app.apply_block_request(
        &orchestrator_request(
            &app,
            1,
            Operation::Visibility {
                panel_id: card.clone(),
                visible: false,
            },
        ),
        &ctx,
    );
    assert!(
        matches!(hidden, Outcome::Visibility { visible: false, .. }),
        "the owner hides its own card: {hidden:?}"
    );

    // close
    let closed = app.apply_block_request(
        &orchestrator_request(&app, 1, Operation::Close { panel_id: card.clone() }),
        &ctx,
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
        app.board.panel_id_by_local_id(&card).is_none(),
        "the card left the board"
    );
}

#[test]
fn a_next_click_that_names_the_card_of_an_orchestrator_waits_for_it() {
    let (_temp, ctx, mut app) = app();
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

#[test]
fn the_buttons_of_a_card_of_an_orchestrator_are_dark_without_its_bridge() {
    let (_temp, ctx, mut app) = app();
    let card = created_in(&mut app, &ctx, 1, "Plan");
    let id = app.board.panel_id_by_local_id(&card).expect("the card");
    assert!(
        !app.card_agent_present(id),
        "no bridge runs for the workspace of this card in a test"
    );
    let chip = app.card_titlebar_chip(id).expect("the card has a state chip");
    assert_eq!(
        chip.state,
        crate::block_widget::chip::ChipState::Gone,
        "the chip of a card without its orchestrator says `No agent`"
    );
    // A press that no wait of the orchestrator reads stays in the queue of the
    // card: the owner has no terminal, so Horizon types no wake line for it.
    press(&mut app, &card, "go");
    assert!(
        !app.settle_block_clicks(),
        "no wake line and no answer leaves this frame for a caller without a panel"
    );
    assert_eq!(
        app.card_pending_clicks(id),
        1,
        "the press waits in the queue of the card for the next wait of its owner"
    );
}
