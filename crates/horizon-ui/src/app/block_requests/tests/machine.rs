//! A card of a session of a machine: its workspace, its presence and its owner,
//! judged on a test board.

use super::*;
use horizon_core::machine_watch::{AgentState, MachineAgent, MachineWatchSnapshot, MachineWatchState, WatchLink};
use horizon_core::{Machine, MachineRegistry, Machines};

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
