//! A jump to a card from its row of the sidebar: the card stands whole in the
//! canvas while the canvas holds it, and it shows its top while it does not.
//!
//! A card of the default width stops at a ceiling that the smallest window
//! holds, so a jump shows it whole. The cards that reach the edge of the canvas
//! are the wide card, which may take the height of the view, and a card that the
//! person made taller by hand.

use egui::{Event, Pos2, Rect};
use horizon_core::browser::manifest::{
    self,
    block::{Operation, Outcome, Request},
};
use horizon_core::{PanelId, PanelKind};

use super::{Window, texts};
use crate::app::test_support::editor_workspace_state;

/// The name of the card, which its row of the sidebar shows.
const CARD: &str = "Tall card";
/// The button at the foot of the card.
const FOOT: &str = "Delete the machine";

/// A document of `rows` pairs, `steps` steps and `lines` lines of text, with one
/// button at its foot, for the wide card when `wide` says so.
fn document(rows: usize, steps: usize, lines: usize, wide: bool) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = (0..rows)
        .map(|index| serde_json::json!({"label": format!("Label {index}"), "value": "value"}))
        .collect();
    let steps: Vec<serde_json::Value> = (0..steps)
        .map(|index| serde_json::json!({"label": format!("Step {index}"), "state": "done"}))
        .collect();
    let mut components = vec![
        serde_json::json!({"type": "heading", "text": "Ready for sample-machine"}),
        serde_json::json!({"type": "kv", "rows": rows}),
        serde_json::json!({"type": "steps", "steps": steps}),
    ];
    components.extend(
        (0..lines).map(|index| serde_json::json!({"type": "text", "text": format!("Line {index} of the notes.")})),
    );
    components.push(serde_json::json!(
        {"type": "buttons", "items": [{"id": "delete", "label": FOOT, "tone": "danger"}]}
    ));
    let mut document = serde_json::json!({ "components": components });
    if wide {
        document["width"] = serde_json::json!("wide");
    }
    document
}

/// A window of `size` points whose one workspace holds an agent panel. A card of
/// that panel stands in the column at its right.
fn agent_window(size: [f32; 2]) -> Window {
    let mut window = Window::new(size, vec![editor_workspace_state("lane", [40.0, 40.0])]);
    window.app.board.panels[0].kind = PanelKind::Claude;
    window
}

/// One request of the agent panel of `window`, as the queue carries it.
fn request(window: &Window, operation: Operation) -> Request {
    Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", window.app.board.panels[0].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The card of `document` that the agent panel of `window` draws, after the
/// frames that give it the height of its content.
fn tall_card(window: &mut Window, document: serde_json::Value) -> PanelId {
    let request = request(
        window,
        Operation::Create {
            title: Some(CARD.into()),
            document,
        },
    );
    let ctx = window.ctx.clone();
    let Outcome::Created { panel_id, .. } = window.app.apply_block_request(&request, &ctx) else {
        panic!("the agent panel draws a card");
    };
    let card = window.app.board.panel_id_by_local_id(&panel_id).expect("the card");
    // The pass that draws a card measures it, so the card stands in the view.
    window.app.reveal_panel_visible(&ctx, card);
    for _ in 0..4 {
        let _ = window.frame(Vec::new());
    }
    card
}

/// The rectangle of the panel `card` in the window.
fn panel_rect(window: &Window, card: PanelId) -> Rect {
    let panel = window.app.board.panel(card).expect("the card");
    let canvas = window.app.canvas_rect(&window.ctx);
    let min = window.app.canvas_to_screen(canvas, Pos2::from(panel.layout.position));
    Rect::from_min_size(
        min,
        window.app.canvas_size_to_screen(egui::Vec2::from(panel.layout.size)),
    )
}

/// Moves the view away from the card and presses the row of the card in the
/// sidebar, in the frames that egui needs for a click. Answers the frame after
/// the jump.
fn press_the_row(window: &mut Window) -> egui::FullOutput {
    window.app.canvas_view.set_pan_offset([-4000.0, -4000.0]);
    let canvas_left = window.app.canvas_rect(&window.ctx).left();
    let drawn = texts(&window.frame(Vec::new()));
    let row = drawn
        .iter()
        .find(|entry| entry.text == CARD && entry.rect.right() < canvas_left)
        .unwrap_or_else(|| panic!("the sidebar draws the row of the card: {drawn:?}"))
        .rect
        .center();
    let button = |pressed| Event::PointerButton {
        pos: row,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for events in [
        vec![Event::PointerMoved(row)],
        vec![button(true)],
        vec![button(false)],
        // The pointer leaves the row, so no hover text stands over the canvas.
        vec![Event::PointerGone],
    ] {
        let _ = window.frame(events);
    }
    let _ = window.frame(Vec::new());
    window.frame(Vec::new())
}

#[test]
fn a_press_on_the_row_of_a_tall_card_shows_the_whole_card_while_the_canvas_holds_it() {
    let mut window = agent_window([1280.0, 900.0]);
    // The view of a jump before this one: the least zoom of a Fit.
    let floor = window.app.template_config.canvas.fit_floor();
    window.app.canvas_view.set_zoom(floor);
    // A wide card takes the height of its content up to the ceiling of a wide
    // card, which is more than this view shows: the card is as tall as the
    // canvas almost, and it scrolls nothing.
    let card = tall_card(&mut window, document(12, 8, 3, true));
    let canvas = window.app.canvas_rect(&window.ctx);
    let height = window.app.board.panel(card).expect("the card").layout.size[1] * floor;
    assert!(
        height <= canvas.height() && height + 40.0 > canvas.height(),
        "the card of {height} points fits the canvas {canvas:?}, and not under the margin of a jump"
    );

    let output = press_the_row(&mut window);

    let (canvas, shown) = (window.app.canvas_rect(&window.ctx), panel_rect(&window, card));
    assert!(
        (window.app.canvas_view.zoom - floor).abs() < 1.0e-4,
        "the jump keeps the least zoom of a Fit: {}",
        window.app.canvas_view.zoom
    );
    assert!(
        shown.top() >= canvas.top() - 0.5 && shown.bottom() <= canvas.bottom() + 0.5,
        "the jump shows the card {shown:?} from its top to its foot in the canvas {canvas:?}"
    );
    let drawn = texts(&output);
    let foot = drawn
        .iter()
        .find(|entry| entry.text == FOOT)
        .unwrap_or_else(|| panic!("the frame draws the button at the foot of the card: {drawn:?}"));
    assert!(
        canvas.contains_rect(foot.rect) && foot.clip.contains_rect(foot.rect),
        "the button at the foot of the card stands in the canvas {canvas:?}: {foot:?}"
    );
}

#[test]
fn a_press_on_the_row_of_a_card_taller_than_the_canvas_shows_its_top() {
    let mut window = agent_window([800.0, 600.0]);
    // A wide card of the ceiling of a wide card, in the smallest window.
    let card = tall_card(&mut window, document(24, 16, 0, true));
    let canvas = window.app.canvas_rect(&window.ctx);
    let floor = window.app.template_config.canvas.fit_floor();
    let height = window.app.board.panel(card).expect("the card").layout.size[1] * floor;
    assert!(
        height > canvas.height(),
        "the card of {height} points is taller than the canvas {canvas:?} at the least zoom of a Fit"
    );

    let _ = press_the_row(&mut window);

    let (canvas, shown) = (window.app.canvas_rect(&window.ctx), panel_rect(&window, card));
    assert!(
        shown.top() >= canvas.top() + 40.0 - 0.5 && shown.top() < canvas.top() + 60.0,
        "the jump shows the top of the card {shown:?} at the margin of the canvas {canvas:?}"
    );
    assert!(
        shown.bottom() > canvas.bottom(),
        "the foot of the card {shown:?} is below the canvas {canvas:?}"
    );
}

/// A card of the default width takes the ceiling of its height at the most, and
/// it stands in the column at the right of its agent panel. A jump shows it whole
/// in every window, and it shows the one line of the card when the card is folded.
#[test]
fn a_press_on_the_row_of_a_card_of_a_column_shows_that_card_open_and_folded() {
    for size in [[800.0, 600.0], [1280.0, 900.0]] {
        let mut window = agent_window(size);
        let card = tall_card(&mut window, document(24, 16, 0, false));
        let agent = window.app.board.panels[0].layout;
        let (place, layout) = {
            let panel = window.app.board.panel(card).expect("the card");
            (panel.block().expect("a card").place, panel.layout)
        };
        assert!(
            place == horizon_core::CardPlace::Column
                && layout.position[0] > agent.position[0] + agent.size[0]
                && (layout.position[1] - agent.position[1]).abs() < 0.5,
            "the card {layout:?} stands in the column at the right of its agent panel {agent:?}: {place:?}"
        );

        let _ = press_the_row(&mut window);
        let (canvas, shown) = (window.app.canvas_rect(&window.ctx), panel_rect(&window, card));
        assert!(
            canvas.contains_rect(shown),
            "the jump shows the whole card {shown:?} of the column in the canvas {canvas:?} at {size:?}"
        );

        // The agent says that the job of the card is finished, so the card folds
        // to one line.
        let mut finished = document(24, 16, 0, false);
        finished["finished"] = serde_json::json!(true);
        let panel_id = window.app.board.panel(card).expect("the card").local_id.clone();
        let ctx = window.ctx.clone();
        let update = request(
            &window,
            Operation::Update {
                panel_id,
                document: finished,
            },
        );
        let _ = window.app.apply_block_request(&update, &ctx);
        for _ in 0..3 {
            let _ = window.frame(Vec::new());
        }
        let line = window.app.board.panel(card).expect("the card").layout.size[1];
        assert!(
            (line - crate::app::PANEL_TITLEBAR_HEIGHT).abs() < 0.5,
            "a folded card is one line high: {line}"
        );
        let _ = press_the_row(&mut window);
        let (canvas, shown) = (window.app.canvas_rect(&window.ctx), panel_rect(&window, card));
        assert!(
            canvas.contains_rect(shown),
            "the jump shows the line {shown:?} of the folded card in the canvas {canvas:?} at {size:?}"
        );
    }
}
