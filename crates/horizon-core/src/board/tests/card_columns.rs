//! The column of the cards of an agent panel, judged on a board without a window.

use crate::layout::TILE_GAP;
use crate::{BlockPanelState, CardPlace, browser_actor};

use super::super::*;
use super::editor_panel_options;

/// A workspace with one agent panel, which waits for its process and starts none.
fn agent_board(layout: Option<WorkspaceLayout>) -> (Board, WorkspaceId, PanelId) {
    let mut board = Board::new();
    let workspace = board.create_workspace("agents");
    if layout.is_none() {
        board.clear_workspace_layout(workspace);
    }
    let options = PanelOptions {
        kind: PanelKind::Claude,
        ..PanelOptions::default()
    };
    let agent = board.create_panel_waiting(options, workspace).expect("an agent panel");
    (board, workspace, agent)
}

/// A card of `agent` at `place`, `height` points high, as the card tool creates it.
fn card(board: &mut Board, agent: PanelId, place: CardPlace, height: f32) -> PanelId {
    let (workspace, actor) = board
        .panel(agent)
        .map(|panel| (panel.workspace_id, browser_actor(&panel.local_id)))
        .expect("the agent panel");
    let state = BlockPanelState {
        owner_actor: Some(actor),
        place,
        ..BlockPanelState::default()
    };
    let options = PanelOptions {
        kind: PanelKind::Block,
        block: Some(state),
        size: Some([420.0, height]),
        ..PanelOptions::default()
    };
    board
        .create_panel_beside_waiting(options, workspace, agent)
        .expect("a card")
}

fn rect(board: &Board, id: PanelId) -> [f32; 4] {
    board.panel_bounds(id).expect("the panel")
}

/// Checks that `cards`, from the top, are the column whose left edge is `left`.
fn assert_column(board: &Board, agent: PanelId, left: f32, cards: &[PanelId]) {
    let mut top = rect(board, agent)[1];
    for card in cards {
        let place = rect(board, *card);
        assert!(
            (place[0] - left).abs() < 0.5 && (place[1] - top).abs() < 0.5,
            "the card stands at {left}, {top}: {place:?}"
        );
        top = place[3] + TILE_GAP;
    }
}

#[test]
fn a_preset_arranges_the_panels_and_leaves_the_cards_in_their_column() {
    let (mut board, workspace, agent) = agent_board(Some(WorkspaceLayout::Grid));
    let tile = rect(&board, agent);
    let first = card(&mut board, agent, CardPlace::Column, 200.0);
    let second = card(&mut board, agent, CardPlace::Column, 150.0);

    assert_eq!(board.workspace_layout_value(workspace), Some(WorkspaceLayout::Grid));
    assert_eq!(rect(&board, agent), tile, "a card takes no tile of the preset");
    assert_column(&board, agent, tile[2] + TILE_GAP, &[second, first]);

    // A second panel takes the tile at the right of the agent, where the column
    // stood, so the column stands at the right of that panel.
    let other = board.create_panel(editor_panel_options(), workspace).expect("a panel");
    assert_eq!(
        rect(&board, other)[0],
        tile[2] + TILE_GAP,
        "the panel takes the next tile"
    );
    assert_column(&board, agent, rect(&board, other)[2] + TILE_GAP, &[second, first]);
}

#[test]
fn a_card_that_grows_or_hides_moves_the_cards_under_it_and_no_other_panel() {
    let (mut board, workspace, agent) = agent_board(None);
    let first = card(&mut board, agent, CardPlace::Column, 200.0);
    let second = card(&mut board, agent, CardPlace::Column, 150.0);
    let left = rect(&board, agent)[2] + TILE_GAP;
    let below = board
        .create_panel(
            PanelOptions {
                position: Some([left, 2_000.0]),
                ..editor_panel_options()
            },
            workspace,
        )
        .expect("a panel under the column");
    let under = rect(&board, below);

    assert!(
        board.resize_panel(second, [420.0, 400.0]),
        "the content of the card grew"
    );
    assert_column(&board, agent, left, &[second, first]);
    assert_eq!(rect(&board, below), under, "a card of a column pushes no panel");

    assert!(board.set_panel_visible(second, false), "the agent hides the card");
    assert_column(&board, agent, left, &[first]);
    assert!(board.set_panel_visible(second, true), "the agent shows the card");
    assert_column(&board, agent, left, &[second, first]);
}

#[test]
fn a_card_of_an_older_board_and_a_card_without_its_agent_keep_their_place() {
    let (mut board, _workspace, agent) = agent_board(None);
    let tile = card(&mut board, agent, CardPlace::Tile, 200.0);
    let place_of_tile = rect(&board, tile);
    let in_column = card(&mut board, agent, CardPlace::Column, 150.0);

    assert_eq!(rect(&board, tile), place_of_tile, "a tile card is in no column");
    assert_column(&board, agent, place_of_tile[2] + TILE_GAP, &[in_column]);

    let place_of_card = rect(&board, in_column);
    let _ = board.close_panel_returning_teardown(agent);
    assert_eq!(
        rect(&board, in_column),
        place_of_card,
        "a card whose agent is gone stays"
    );
}

#[test]
fn a_card_that_moves_to_another_workspace_is_a_tile_there() {
    let (mut board, _workspace, agent) = agent_board(None);
    let first = card(&mut board, agent, CardPlace::Column, 200.0);
    let second = card(&mut board, agent, CardPlace::Column, 150.0);
    let other = board.create_workspace("other");

    board.assign_panel_to_workspace(second, other);

    let place = board.panel(second).and_then(Panel::block).map(|card| card.place);
    assert_eq!(place, Some(CardPlace::Tile), "the card is a tile of its new workspace");
    assert_eq!(board.workspace_layout_value(other), Some(WorkspaceLayout::Grid));
    assert_column(&board, agent, rect(&board, agent)[2] + TILE_GAP, &[first]);
}

#[test]
fn a_tile_card_that_leaves_the_tiles_leaves_no_hole_in_the_preset() {
    let (mut board, workspace, agent) = agent_board(Some(WorkspaceLayout::Grid));
    let tile = card(&mut board, agent, CardPlace::Tile, 200.0);
    let other = board.create_panel(editor_panel_options(), workspace).expect("a panel");
    let third_tile = rect(&board, other);

    // The card folds to one line, as the fold of the application does it.
    board
        .panel_mut(tile)
        .and_then(Panel::block_mut)
        .expect("the card")
        .place = CardPlace::Free;
    assert!(board.resize_panel(tile, [420.0, 34.0]), "the card takes one line");

    assert_eq!(board.workspace_layout_value(workspace), Some(WorkspaceLayout::Grid));
    assert_ne!(
        rect(&board, other),
        third_tile,
        "the preset closes the hole of the card"
    );
    assert_eq!(rect(&board, other)[0], rect(&board, agent)[2] + TILE_GAP);
}
