//! The identity of a panel, from the frame that creates it to the process that it
//! starts. An agent program reads its browser actor from the local id of its
//! panel, and the board answers a request of that actor with the same panel, so
//! the preparation of a start must read one value and not another.

use crate::panel::{PanelKind, PanelOptions};

use super::super::Board;

/// The kinds of agent panel that Set up can start.
const SETUP_KINDS: [PanelKind; 5] = [
    PanelKind::Codex,
    PanelKind::Claude,
    PanelKind::Pi,
    PanelKind::Grok,
    PanelKind::OpenCode,
];

fn agent_options(kind: PanelKind) -> PanelOptions {
    PanelOptions {
        kind,
        command: Some("/bin/sh".to_owned()),
        args: vec!["-c".to_owned(), "exit 0".to_owned()],
        ..PanelOptions::default()
    }
}

#[test]
fn a_panel_that_waits_for_its_start_prepares_under_its_own_local_id() {
    for kind in SETUP_KINDS {
        let mut board = Board::new();
        let workspace = board.create_workspace("agents");
        let id = board
            .create_panel_waiting(agent_options(kind), workspace)
            .expect("an agent panel");
        let local_id = board.panel(id).expect("the panel").local_id.clone();
        assert!(!local_id.is_empty(), "{kind:?} names its panel");
        let pending = board.take_pending_starts();
        let pending = pending.first().expect("the panel waits for its start");
        assert_eq!(pending.id, id, "{kind:?} prepares the panel that stands");
        assert_eq!(
            pending.options.local_id.as_deref(),
            Some(local_id.as_str()),
            "the preparation of a {kind:?} panel reads the local id of its panel"
        );
    }
}

#[test]
fn two_panels_of_one_kind_hold_two_local_ids() {
    let mut board = Board::new();
    let workspace = board.create_workspace("agents");
    let first = board
        .create_panel_waiting(agent_options(PanelKind::Codex), workspace)
        .expect("the first agent panel");
    let second = board
        .create_panel_waiting(agent_options(PanelKind::Codex), workspace)
        .expect("the second agent panel");
    let first_local_id = board.panel(first).expect("the first panel").local_id.clone();
    let second_local_id = board.panel(second).expect("the second panel").local_id.clone();
    assert_ne!(
        first_local_id, second_local_id,
        "two panels of one kind in one folder are two callers"
    );
    assert_ne!(
        crate::panel::browser_actor(&first_local_id),
        crate::panel::browser_actor(&second_local_id),
        "the actor of a panel names one panel and no other"
    );
}
