//! A card takes the height its content needs, once per document and per set of
//! Horizon-owned frames.

use horizon_browser_control::block::BlockDocument;
use horizon_core::{BlockPanelState, CardFrames, Panel, PanelKind, PanelOptions, RuntimeState, StartupDecision};

use super::*;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use crate::block_widget::CARD_CHROME_HEIGHT;

/// A board with one card panel that measured `measured_content_height`.
fn app_with_card(measured_content_height: f32) -> (tempfile::TempDir, Context, HorizonApp, PanelId) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("first", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    let workspace = app.board.panels[0].workspace_id;
    let id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(BlockPanelState::new(BlockDocument {
                    width: None,
                    components: Vec::new(),
                })),
                size: Some([420.0, 220.0]),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a card panel spawns");
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .observe_content_height(measured_content_height);
    (temp, ctx, app, id)
}

fn height(app: &HorizonApp, id: PanelId) -> f32 {
    app.board.panel(id).expect("the card").layout.size[1]
}

#[test]
fn a_card_takes_the_height_of_the_content_that_the_pass_measured() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (600.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the card fits its content: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_stays_between_the_card_limits() {
    let (_short, ctx, mut short, short_id) = app_with_card(5.0);
    short.fit_block_cards(&ctx);
    assert!((height(&short, short_id) - 140.0).abs() < f32::EPSILON, "the floor");

    let (_tall, ctx, mut tall, tall_id) = app_with_card(5_000.0);
    tall.fit_block_cards(&ctx);
    assert!((height(&tall, tall_id) - 900.0).abs() < f32::EPSILON, "the ceiling");
}

#[test]
fn one_fit_per_document_leaves_a_settled_card_alone() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.fit_block_cards(&ctx);
    assert!(app.board.resize_panel(id, [420.0, 320.0]), "the card is resized");

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - 320.0).abs() < f32::EPSILON,
        "the fitted revision does not resize the card again: {}",
        height(&app, id)
    );
}

#[test]
fn a_new_document_fits_the_card_again() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.fit_block_cards(&ctx);

    let state = app.board.panel_mut(id).and_then(Panel::block_mut).expect("a card");
    let _ = state.replace_document(BlockDocument {
        width: None,
        components: Vec::new(),
    });
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .observe_content_height(300.0);
    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (300.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the next revision fits its own content: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_that_the_person_resized_keeps_its_size() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .mark_hand_resized();

    app.fit_block_cards(&ctx);
    assert!(
        (height(&app, id) - 220.0).abs() < f32::EPSILON,
        "the person's size stays"
    );

    let state = app.board.panel_mut(id).and_then(Panel::block_mut).expect("a card");
    let _ = state.replace_document(BlockDocument {
        width: None,
        components: Vec::new(),
    });
    app.panel_render_caches
        .card_runtime
        .entry(id)
        .or_default()
        .observe_content_height(300.0);
    app.fit_block_cards(&ctx);
    assert!(
        (height(&app, id) - 220.0).abs() < f32::EPSILON,
        "a later document does not undo the person's size: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_fits_again_when_a_frame_of_the_host_appears_or_goes() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    app.fit_block_cards(&ctx);
    let fitted = height(&app, id);

    // The owner leaves: Horizon puts its frame at the top of the body, so the
    // body needs those points on top of the document.
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.observe_frames(CardFrames {
        agent_gone: true,
        clicks_refused: false,
        link_refused: false,
    });
    card.observe_content_height(700.0);
    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (700.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the card grows for the frame of a gone agent: {} was {fitted}",
        height(&app, id)
    );

    // The owner comes back: the frame is gone, so the card takes the height of
    // its document again.
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.observe_frames(CardFrames::default());
    card.observe_content_height(600.0);
    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - (600.0 + CARD_CHROME_HEIGHT)).abs() < f32::EPSILON,
        "the card drops the room of the frame when the agent is back: {}",
        height(&app, id)
    );
}

#[test]
fn a_card_that_the_person_resized_keeps_its_size_when_a_frame_appears() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    let card = app.panel_render_caches.card_runtime.entry(id).or_default();
    card.mark_hand_resized();
    card.observe_frames(CardFrames {
        agent_gone: true,
        clicks_refused: false,
        link_refused: false,
    });

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - 220.0).abs() < f32::EPSILON,
        "the person's size outranks the frame: {}",
        height(&app, id)
    );
}

#[test]
fn a_workspace_preset_keeps_the_size_of_its_cards() {
    let (_temp, ctx, mut app, id) = app_with_card(600.0);
    let workspace = app.board.panel(id).expect("the card").workspace_id;
    app.board
        .arrange_workspace(workspace, horizon_core::WorkspaceLayout::Rows);
    let arranged = height(&app, id);
    let sibling = app
        .board
        .workspace(workspace)
        .expect("workspace")
        .panels
        .iter()
        .copied()
        .find(|panel| *panel != id)
        .expect("a sibling panel");
    let sibling_height = height(&app, sibling);

    app.fit_block_cards(&ctx);

    assert!(
        (height(&app, id) - arranged).abs() < f32::EPSILON,
        "the preset owns the size of its panels"
    );
    assert!((height(&app, sibling) - sibling_height).abs() < f32::EPSILON);
}
