//! A card of a session of a machine: its workspace, its presence and its owner,
//! judged on a test board.

use super::*;
use crate::app::test_support::{card_body, card_panel, drawn_text, drew_text, raw_input, run_app_frame_with_input};
use horizon_core::machine_watch::{AgentState, MachineAgent, MachineWatchSnapshot, MachineWatchState, WatchLink};
use horizon_core::{Machine, MachineRegistry, Machines, WorkspaceLayout};

/// Keeps the machine `build-1` in the list of the app, with the mark of its cards.
fn keep_machine(app: &mut HorizonApp) {
    let machine = Machine {
        name: "build-1".to_owned(),
        host: "127.0.0.1".to_owned(),
        user: None,
        port: Some(2222),
        session: None,
        provider: None,
        repository: None,
        added_at_millis: 0,
        cards: true,
    };
    app.machine_list = Machines::with_records(MachineRegistry::from_home(app.session_store.home()), vec![machine]);
}

/// Keeps a watch snapshot that reports the session `session` of `build-1`, or none.
fn watch_reports(app: &mut HorizonApp, session: Option<&str>) {
    let agents: Vec<MachineAgent> = session
        .map(|session| MachineAgent {
            machine: "build-1".to_owned(),
            session: session.to_owned(),
            pane: "%1".to_owned(),
            kind: PanelKind::Claude,
            name: "night".to_owned(),
            state: AgentState::default(),
        })
        .into_iter()
        .collect();
    app.machine_watch.snapshot = Some(std::sync::Arc::new(MachineWatchSnapshot {
        version: 1,
        machines: vec![MachineWatchState {
            machine: "build-1".to_owned(),
            link: WatchLink::Watching,
            agents,
        }],
        ..MachineWatchSnapshot::default()
    }));
}

/// A request of the session `session` of `build-1`, whose agent runs there.
fn session_request(_app: &HorizonApp, session: &str, operation: Operation) -> Request {
    Request {
        request_id: "test-session".into(),
        actor: horizon_core::node_cards::actor("build-1", session).expect("a plain pair"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The window of the person who reads the card.
const WINDOW: [f32; 2] = [1480.0, 900.0];

/// The document of the card that the stand-in agent of the machine draws
/// (`scripts/device-smoke/machine_card_agent.py`): a heading with a subtitle, one
/// text of two lines and one row of two buttons.
fn fixture_card_document() -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": "Night shift", "subtitle": "an agent on the machine"},
            {"type": "text", "text": "This card came through the socket of the session, not from a panel of Horizon."},
            {"type": "buttons", "items": [
                {"id": "continue", "label": "Continue", "tone": "primary"},
                {"id": "stop", "label": "Stop", "tone": "danger"}]}
        ]
    })
}

/// Runs the frames that the layout of a new card needs: one pass measures the body,
/// the host sizes the card, and the next pass draws the sized card. Returns the last
/// frame, which is the one that the person sees.
fn settle_frames(ctx: &Context, app: &mut HorizonApp) -> egui::FullOutput {
    let mut output = run_app_frame_with_input(ctx, app, raw_input(WINDOW, None));
    for _ in 0..2 {
        output = run_app_frame_with_input(ctx, app, raw_input(WINDOW, None));
    }
    output
}

#[test]
fn a_request_of_a_machine_session_opens_a_card_in_the_workspace_of_its_machine() {
    let (_temp, ctx, mut app) = app();
    keep_machine(&mut app);
    watch_reports(&mut app, Some("night"));
    let request = session_request(
        &app,
        "night",
        Operation::Create {
            title: Some("night shift".into()),
            document: document("night shift"),
        },
    );
    let (card, _revision) = created(app.apply_block_request(&request, &ctx));
    let panel = app
        .board
        .panel_id_by_local_id(&card)
        .and_then(|id| app.board.panel(id))
        .expect("the card is on the board");
    let workspace = app
        .board
        .workspaces
        .iter()
        .find(|workspace| workspace.id == panel.workspace_id)
        .expect("the workspace of the card");
    assert_eq!(
        workspace.name, "build-1",
        "the card lands in the workspace of the machine"
    );
    assert_eq!(card_owner(panel), Some(request.actor.as_str()));
    assert!(
        !app.board.panels.iter().any(|panel| panel.kind == PanelKind::Ssh),
        "no terminal of that session had to be open"
    );
}

#[test]
fn a_card_of_a_machine_session_that_fits_shows_its_last_button_without_a_mark() {
    let (_temp, ctx, mut app) = app();
    // The watch of these frames reads this computer's `ssh`, so it stays off; the card
    // then keeps the frame of an owner that is not there, and the fit counts that frame
    // as it counts the document.
    app.template_config.machines.watch_agents = false;
    keep_machine(&mut app);
    watch_reports(&mut app, Some("night"));
    let (card, _) = created(app.apply_block_request(
        &session_request(
            &app,
            "night",
            Operation::Create {
                title: Some("Night shift".into()),
                document: fixture_card_document(),
            },
        ),
        &ctx,
    ));
    assert!(
        !app.board.panels.iter().any(|panel| panel.kind == PanelKind::Ssh),
        "no terminal of that session had to be open"
    );
    let workspace = app
        .board
        .panel_id_by_local_id(&card)
        .and_then(|id| app.board.panel(id))
        .expect("the card")
        .workspace_id;
    assert!(
        app.board
            .workspace(workspace)
            .is_some_and(|workspace| workspace.layout.is_none()),
        "the workspace of the cards of a machine starts free, so its cards fit themselves"
    );
    // The person looks at the workspace of the machine, so the frames draw the card. The
    // first frame applies the view of the startup state, and the next ones measure the
    // card and draw it at the size that the host gave it.
    let _ = run_app_frame_with_input(&ctx, &mut app, raw_input(WINDOW, None));
    let canvas = app.canvas_rect(&ctx);
    assert!(app.center_workspace_in_rect(workspace, canvas));

    let output = settle_frames(&ctx, &mut app);
    let body = card_body(card_panel(&app, &ctx, &card), app.canvas_view.zoom);
    let last = drawn_text(&output, "Stop");
    assert!(
        body.contains_rect(last),
        "the last button of the card lies inside the room that the card gives its body: {last:?} in {body:?}"
    );
    assert!(!drew_text(&output, "More below"), "a card that fits draws no mark");
}

#[test]
fn a_layout_that_the_person_chose_keeps_the_cards_of_a_machine_in_its_cells() {
    let (_temp, ctx, mut app) = app();
    keep_machine(&mut app);
    watch_reports(&mut app, Some("night"));
    let (card, _) = created(app.apply_block_request(
        &session_request(
            &app,
            "night",
            Operation::Create {
                title: Some("Night shift".into()),
                document: fixture_card_document(),
            },
        ),
        &ctx,
    ));
    let workspace = app
        .board
        .panel_id_by_local_id(&card)
        .and_then(|id| app.board.panel(id))
        .expect("the card")
        .workspace_id;
    app.board.arrange_workspace(workspace, WorkspaceLayout::Rows);

    // A second card of that machine joins the arrangement that the person chose.
    let _ = created(app.apply_block_request(
        &session_request(
            &app,
            "night",
            Operation::Create {
                title: Some("Night notes".into()),
                document: fixture_card_document(),
            },
        ),
        &ctx,
    ));

    assert_eq!(
        app.board.workspace(workspace).and_then(|workspace| workspace.layout),
        Some(WorkspaceLayout::Rows),
        "the preset that the person chose stays theirs"
    );
}

#[test]
fn a_card_of_a_machine_session_names_its_owner_present_and_after_absent() {
    let (_temp, ctx, mut app) = app();
    keep_machine(&mut app);
    watch_reports(&mut app, Some("night"));
    let create = session_request(
        &app,
        "night",
        Operation::Create {
            title: Some("night shift".into()),
            document: document("night shift"),
        },
    );
    let (card, _revision) = created(app.apply_block_request(&create, &ctx));
    let inspect = session_request(&app, "night", Operation::Inspect { panel_id: card.clone() });
    let Outcome::Inspected {
        agent_present,
        owned_by_caller,
        ..
    } = app.apply_block_request(&inspect, &ctx)
    else {
        panic!("inspect answers");
    };
    assert!(agent_present, "the watch reports the session of the owner");
    assert!(owned_by_caller);

    watch_reports(&mut app, None);
    let Outcome::Inspected { agent_present, .. } = app.apply_block_request(&inspect, &ctx) else {
        panic!("inspect answers");
    };
    assert!(!agent_present, "a session that the watch no longer reports is absent");
}
