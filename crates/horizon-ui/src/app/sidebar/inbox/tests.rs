//! The group `Needs you` of the sidebar: what counts as a question, the order of
//! the questions, the press of an answer on the one path of a card, the reveal of
//! the card, the cut of the text of a program, the rectangles of an item, and the
//! reads that a frame of the sidebar makes.

use egui::{Color32, Context};
use horizon_browser_control::block::{BlockDocument, Button, Component, Tone};
use horizon_core::{PanelId, PanelKind, PanelOptions, WorkspaceId, agent_bridge::actor};

use crate::app::test_support::{raw_input, test_app};
use crate::app::{HorizonApp, sidebar::SidebarActions};
use crate::test_egui::DiscardTextures;

use super::item::ItemRects;
use super::{MAX_ITEMS, MAX_LABEL_CHARS, MAX_TEXT_CHARS, Question};

/// The sidebar of the window, which the item of a question stands in.
const VIEWPORT: [f32; 2] = [1200.0, 900.0];

/// A card that asks `text` with `answers` in one callout.
fn asking_card(text: &str, answers: &[(&str, &str)]) -> BlockDocument {
    BlockDocument {
        width: None,
        finished: false,
        components: vec![
            Component::Heading {
                text: "Cold start".to_owned(),
                subtitle: None,
                badge: None,
            },
            callout(text, answers),
        ],
    }
}

/// One callout with the answers of `answers`.
fn callout(text: &str, answers: &[(&str, &str)]) -> Component {
    Component::Callout {
        tone: Tone::Neutral,
        caption: None,
        text: text.to_owned(),
        buttons: answers
            .iter()
            .map(|(id, label)| Button {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
                tone: Tone::Neutral,
            })
            .collect(),
    }
}

/// A card panel of `workspace` that draws `document`, owned by the orchestrator of
/// the workspace as `updated_at_millis` says.
fn card_panel(app: &mut HorizonApp, workspace: WorkspaceId, document: BlockDocument) -> PanelId {
    card_panel_at(app, workspace, document, 0)
}

/// [`card_panel`], with the time of the last update of its document.
fn card_panel_at(app: &mut HorizonApp, workspace: WorkspaceId, document: BlockDocument, at: i64) -> PanelId {
    let local_id = app.board.workspace(workspace).expect("workspace").local_id.clone();
    // A question is the question of a card whose owner is here: these tests hold a
    // program of the dock without starting one.
    app.orchestrator_dock.mark_running_for_test(&local_id);
    let state = horizon_core::BlockPanelState {
        title: "Cold start".to_owned(),
        owner_actor: actor::of_workspace(&local_id),
        updated_at_millis: at,
        ..horizon_core::BlockPanelState::new(document)
    };
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(state),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("card")
}

/// The answer of the callout of a card that asks `text` with `answers`.
fn answer_of(app: &HorizonApp, panel: PanelId) -> Component {
    let card = app.board.panel(panel).and_then(|panel| panel.block()).expect("card");
    card.document
        .components
        .iter()
        .find(|component| matches!(component, Component::Callout { .. }))
        .cloned()
        .expect("the card asks a question")
}

/// The questions of the board, as the sidebar reads them.
fn questions_of(app: &HorizonApp) -> Vec<Question> {
    app.read_questions()
}

/// One frame of the sidebar with `events`.
fn sidebar_frame(ctx: &Context, app: &mut HorizonApp, events: Vec<egui::Event>) {
    let mut input = raw_input(VIEWPORT, None);
    input.events = events;
    let _ = ctx.run_ui(input, |ui| app.render_sidebar(ui.ctx())).discard_textures();
}

/// Presses the sidebar where it draws the text that starts with `prefix`.
fn press(platform: &Context, app: &mut HorizonApp, prefix: &str) {
    let mut at = None;
    // A row shows from its second frame on, so the text is there before the press.
    for _ in 0..2 {
        at = drawn_text_center(platform, app, prefix);
    }
    let position = at.unwrap_or_else(|| panic!("the sidebar draws the text {prefix:?}"));
    let button = |pressed: bool| egui::Event::PointerButton {
        pos: position,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    sidebar_frame(platform, app, vec![egui::Event::PointerMoved(position)]);
    sidebar_frame(platform, app, vec![button(true)]);
    sidebar_frame(platform, app, vec![button(false)]);
}

/// The centre of the first text that one frame of the sidebar draws with `prefix`.
fn drawn_text_center(platform: &Context, app: &mut HorizonApp, prefix: &str) -> Option<egui::Pos2> {
    let output = platform
        .run_ui(raw_input(VIEWPORT, None), |ui| app.render_sidebar(ui.ctx()))
        .discard_textures();
    let mut drawn = Vec::new();
    for clipped in &output.shapes {
        collect_text(&clipped.shape, &mut drawn);
    }
    drawn
        .into_iter()
        .find(|(text, _)| text.starts_with(prefix))
        .map(|(_, rect)| rect.center())
}

/// The texts of `shape` with the rectangle of each, in draw order.
fn collect_text(shape: &egui::Shape, drawn: &mut Vec<(String, egui::Rect)>) {
    match shape {
        egui::Shape::Text(text) => drawn.push((
            text.galley.text().to_owned(),
            egui::Rect::from_min_size(text.pos, text.galley.size()),
        )),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text(shape, drawn);
            }
        }
        _ => {}
    }
}

#[test]
fn only_a_callout_that_nobody_answered_of_an_open_card_is_a_question() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");

    // A card without a callout, and a callout without an answer.
    let plain = card_panel(
        &mut app,
        workspace,
        BlockDocument {
            width: None,
            finished: false,
            components: vec![Component::Text {
                text: "the plan runs".to_owned(),
            }],
        },
    );
    let no_answer = card_panel(&mut app, workspace, asking_card("Nothing to answer", &[]));
    assert!(questions_of(&app).is_empty(), "no card asks a question yet");

    // A finished document, and a folded card, ask nothing.
    let mut finished = asking_card("Run the plan?", &[("go", "Go")]);
    finished.finished = true;
    let done = card_panel(&mut app, workspace, finished);
    let mut folded = horizon_core::BlockPanelState::new(asking_card("Run the plan?", &[("go", "Go")]));
    folded.fold = Some(true);
    let folded_card = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(horizon_core::BlockPanelState {
                    owner_actor: actor::of_workspace(&app.board.workspace(workspace).expect("workspace").local_id),
                    ..folded
                }),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("card");
    assert!(
        questions_of(&app).is_empty(),
        "a finished document and a folded card ask nothing"
    );
    assert_ne!(plain, no_answer);
    assert_ne!(done, folded_card);

    // A card that is answered is no question, also while the card stays open.
    let asked = card_panel(&mut app, workspace, asking_card("Run the plan?", &[("go", "Go")]));
    assert_eq!(questions_of(&app).len(), 1);
    let callout = answer_of(&app, asked);
    app.panel_render_caches
        .card_runtime
        .entry(asked)
        .or_default()
        .record_answer(callout.clone());
    assert!(
        questions_of(&app).is_empty(),
        "the answer of the person ends the question"
    );

    // The saved card holds the answer after a restart, and the record of this run
    // ends it too, on the rule of the attention of a card.
    app.board
        .panel_mut(asked)
        .and_then(horizon_core::Panel::block_mut)
        .expect("card")
        .record_answer(callout);
    app.panel_render_caches.card_runtime.remove(&asked);
    assert!(questions_of(&app).is_empty());

    // A closed card asks nothing.
    app.board.close_panel(asked);
    assert!(questions_of(&app).is_empty());
}

#[test]
fn a_card_whose_owner_is_gone_is_no_question() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    let local_id = app.board.workspace(workspace).expect("workspace").local_id.clone();

    // The card asks a question while no program of the dock runs for its workspace:
    // nobody reads the answer, so the group shows no question and its buttons stay
    // out of the reach of the person.
    let state = horizon_core::BlockPanelState {
        owner_actor: actor::of_workspace(&local_id),
        ..horizon_core::BlockPanelState::new(asking_card("Run the plan?", &[("go", "Go")]))
    };
    app.board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Block,
                block: Some(state),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("card");
    assert!(
        !app.orchestrator_dock.running(&local_id),
        "no program of the dock runs for this workspace"
    );
    assert!(
        questions_of(&app).is_empty(),
        "the answer of a card whose owner is gone reaches no program"
    );

    // The program of that workspace runs again, and the same card is a question.
    app.orchestrator_dock.mark_running_for_test(&local_id);
    assert_eq!(questions_of(&app).len(), 1, "the owner is here, so the question counts");
}

#[test]
fn the_first_unanswered_callout_of_a_card_is_its_question() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    let document = BlockDocument {
        width: None,
        finished: false,
        components: vec![
            callout("Two ways cut the start time. Pick one.", &[("lazy", "Lazy fonts")]),
            callout("Push the branch?", &[("push", "Push")]),
        ],
    };
    let card = card_panel(&mut app, workspace, document);
    let questions = questions_of(&app);
    assert_eq!(questions.len(), 1, "one card asks one question");
    assert_eq!(questions[0].panel, card);
    assert_eq!(questions[0].text, "Two ways cut the start time. Pick one.");

    // The person answers the first callout, so the second one is the question.
    let first = questions[0].clone();
    let (first_callout, second_callout) = (callout_at(&app, card, 0), callout_at(&app, card, 1));
    app.panel_render_caches
        .card_runtime
        .entry(card)
        .or_default()
        .record_answer(first_callout.clone());
    let questions = questions_of(&app);
    assert_eq!(questions.len(), 1, "the card asks its other callout");
    assert_eq!(questions[0].text, "Push the branch?");
    assert_ne!(questions[0].text, first.text);
    assert_ne!(second_callout, first_callout);
}

/// The callout of component `index` of the document of `panel`.
fn callout_at(app: &HorizonApp, panel: PanelId, index: usize) -> Component {
    let card = app.board.panel(panel).and_then(|panel| panel.block()).expect("card");
    card.document
        .components
        .iter()
        .filter(|component| matches!(component, Component::Callout { .. }))
        .nth(index)
        .cloned()
        .expect("the callout")
}

#[test]
fn the_questions_come_oldest_first() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    let young = card_panel_at(&mut app, workspace, asking_card("Young", &[("go", "Go")]), 500);
    let old = card_panel_at(&mut app, workspace, asking_card("Old", &[("go", "Go")]), 100);
    let same = card_panel_at(&mut app, workspace, asking_card("Same time", &[("go", "Go")]), 100);

    let order: Vec<PanelId> = questions_of(&app).iter().map(|question| question.panel).collect();
    assert_eq!(
        order,
        vec![old, same, young],
        "the oldest update of a card comes first, and the board orders two of one time"
    );
    assert_eq!(questions_of(&app)[0].text, "Old");
}

#[test]
fn the_group_draws_three_questions_and_names_the_rest() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    for index in 0..5 {
        card_panel_at(
            &mut app,
            workspace,
            asking_card(&format!("Question {index}"), &[("go", "Go")]),
            i64::from(index),
        );
    }
    let questions = questions_of(&app);
    assert_eq!(questions.len(), 5);
    assert_eq!(
        questions
            .iter()
            .take(MAX_ITEMS)
            .map(|question| question.text.as_str())
            .collect::<Vec<_>>(),
        ["Question 0", "Question 1", "Question 2"],
        "the three oldest questions stand in the group"
    );
    assert_eq!(MAX_ITEMS, 3);
}

#[test]
fn a_press_on_an_answer_of_the_group_is_the_press_of_the_card() {
    let (_temp, mut app) = test_app();
    let platform = Context::default();
    let workspace = app.board.create_workspace("Lane");
    let card = card_panel(
        &mut app,
        workspace,
        asking_card(
            "Two ways cut the start time. Pick one.",
            &[("lazy", "Lazy fonts"), ("format", "New format")],
        ),
    );
    let other = card_panel(
        &mut app,
        workspace,
        asking_card(
            "Two ways cut the start time. Pick one.",
            &[("fast", "Fast start"), ("format", "New format")],
        ),
    );
    app.refresh_sidebar_rows(std::time::Instant::now());
    assert_eq!(app.sidebar_list.questions.len(), 2, "the group holds both questions");

    // A press on the button of the card records the click and the answer there.
    let document = app
        .board
        .panel(card)
        .and_then(|panel| panel.block())
        .expect("card")
        .document
        .clone();
    let mut press_on_card = horizon_core::CardRuntime::default();
    assert!(press_on_card.press("lazy", 1, &document, horizon_browser_control::manifest::now_millis()));
    let answer_of_the_card = press_on_card.answered_callout().cloned();

    // The press on the same answer of the group goes through that one path.
    press(&platform, &mut app, "Lazy fonts");
    let runtime = app
        .panel_render_caches
        .card_runtime
        .get_mut(&card)
        .expect("the runtime of the card");
    assert_eq!(
        runtime.answered_callout().cloned(),
        answer_of_the_card,
        "the group records the answer that a press on the card records"
    );
    assert_eq!(
        runtime.pending_clicks(horizon_browser_control::manifest::now_millis()),
        1,
        "the click of the person waits for the agent of the card"
    );

    // The question leaves the group on the next read.
    app.refresh_sidebar_rows(std::time::Instant::now());
    let left: Vec<PanelId> = app
        .sidebar_list
        .questions
        .iter()
        .map(|question| question.panel)
        .collect();
    assert!(!left.contains(&card), "the answered question left the group: {left:?}");
    assert!(left.contains(&other), "the question of the other card stays");
}

#[test]
fn a_press_beside_the_buttons_shows_the_card() {
    let (_temp, mut app) = test_app();
    let platform = Context::default();
    let front = app.board.create_workspace("Front");
    let lane = app.board.create_workspace("Lane");
    let card = card_panel(&mut app, lane, asking_card("Run the plan?", &[("go", "Go")]));
    app.board.focus_workspace(front);
    app.refresh_sidebar_rows(std::time::Instant::now());

    press(&platform, &mut app, "Run the plan?");
    assert_eq!(
        app.board.active_workspace,
        Some(lane),
        "the workspace of the card becomes the active one"
    );
    assert_eq!(app.board.focused, Some(card), "the canvas shows the card");
}

#[test]
fn the_text_of_a_program_is_plain_and_cut_before_the_item_lays_it_out() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    let long = "x".repeat(MAX_TEXT_CHARS + 40);
    let label = "y".repeat(MAX_LABEL_CHARS + 9);
    let document = BlockDocument {
        width: None,
        finished: false,
        components: vec![callout(
            &format!("line\u{7}break\nof\u{1b}[31m color {long}"),
            &[("lazy", &label), ("format", "New format")],
        )],
    };
    card_panel(&mut app, workspace, document);
    let questions = questions_of(&app);
    let question = questions.first().expect("the question");
    assert!(
        !question.text.chars().any(char::is_control),
        "no control character reaches the item"
    );
    assert!(
        question.text.starts_with("line break of [31m color "),
        "{}",
        question.text
    );
    assert_eq!(question.text.chars().count(), MAX_TEXT_CHARS, "the text is cut");
    assert_eq!(
        question.answers[0].label.chars().count(),
        MAX_LABEL_CHARS,
        "the label is cut"
    );
    assert_eq!(question.answers[1].label, "New format");
    assert_eq!(
        question.spoken,
        format!("Lane: {}", question.text),
        "a screen reader says the workspace and the question"
    );
}

#[test]
fn an_item_of_a_sidebar_214_wide_keeps_its_inset_its_padding_and_its_row_of_buttons() {
    let platform = Context::default();
    let mut rects = None;
    let _ = platform
        .run_ui(raw_input(VIEWPORT, None), |ui| {
            let width = |text: &str, weight: f32| {
                crate::app::sidebar::text::line(ui, text, 11.5, weight, Color32::WHITE, 200.0)
                    .size()
                    .x
                    + 2.0 * 10.0
            };
            let widths = [
                width("Lazy fonts", crate::app::sidebar::text::SEMIBOLD),
                width("New format", crate::app::sidebar::text::REGULAR),
            ];
            rects = Some(ItemRects::of(0.0, 214.0, 14.0, 28.0, &widths));
        })
        .discard_textures();
    let rects = rects.expect("the rectangles of one item");

    assert!(
        (rects.box_rect.min.x - 8.0).abs() <= f32::EPSILON,
        "the box starts 8 in"
    );
    assert!(
        (rects.box_rect.max.x - (214.0 - 8.0)).abs() <= f32::EPSILON,
        "the box ends 8 in"
    );
    assert!(
        (rects.name_rect.min.x - 18.0).abs() <= f32::EPSILON,
        "the text starts 10 in the box"
    );
    assert!((rects.name_rect.max.x - (214.0 - 18.0)).abs() <= f32::EPSILON);
    assert!(
        (rects.text_rect.min.y - (rects.name_rect.max.y + 7.0)).abs() <= f32::EPSILON,
        "line 2 stands 7 under line 1"
    );
    let first = rects.buttons.get(0).expect("the first answer");
    let second = rects.buttons.get(1).expect("the second answer");
    assert!(
        (first.min.x - rects.name_rect.min.x).abs() <= f32::EPSILON,
        "the first button starts the row"
    );
    assert!((first.height() - 26.0).abs() <= f32::EPSILON, "a button is 26 high");
    assert!(
        (second.min.x - (first.max.x + 6.0)).abs() <= f32::EPSILON,
        "the second button follows the first with 6 between them: {first:?} {second:?}"
    );
    assert!(
        (second.min.y - first.min.y).abs() <= f32::EPSILON,
        "two answers share one row"
    );
    assert_eq!(rects.buttons.count(), 2);
    assert!(
        (rects.height() - (first.max.y + 10.0)).abs() <= f32::EPSILON,
        "the box keeps 10 under its last row of buttons"
    );

    // A label wider than the content keeps the width of the content.
    let wide = ItemRects::of(0.0, 214.0, 14.0, 28.0, &[300.0]);
    assert!(
        (wide.buttons.get(0).expect("a button").max.x - (214.0 - 18.0)).abs() <= f32::EPSILON,
        "a wide button fills the content and no more"
    );

    // Two answers that do not fit one row stack, and so does `Open` when it does
    // not fit beside the second one.
    let stacked = ItemRects::of(0.0, 214.0, 14.0, 28.0, &[100.0, 100.0, 40.0]);
    let first = stacked.buttons.get(0).expect("the first answer");
    let second = stacked.buttons.get(1).expect("the second answer");
    let third = stacked.buttons.get(2).expect("the control that opens the card");
    assert!(
        (second.min.y - (first.max.y + 6.0)).abs() <= f32::EPSILON,
        "the second goes under the first"
    );
    assert!((second.min.x - 18.0).abs() <= f32::EPSILON);
    assert!(
        (third.min.y - second.min.y).abs() <= f32::EPSILON,
        "the open control fits beside it"
    );
    assert!((third.min.x - (second.max.x + 6.0)).abs() <= f32::EPSILON);
    assert_eq!(stacked.buttons.count(), 3);
}

#[test]
fn the_group_takes_no_room_while_no_card_asks_a_question() {
    let (_temp, mut app) = test_app();
    let platform = Context::default();
    let workspace = app.board.create_workspace("Lane");
    let (drawn, height) = render_inbox(&platform, &mut app);
    assert!(!drawn, "no question, so no group");
    assert!(height <= f32::EPSILON, "the group takes no room: {height}");

    card_panel(&mut app, workspace, asking_card("Run the plan?", &[("go", "Go")]));
    app.refresh_sidebar_rows(std::time::Instant::now());
    let (drawn, height) = render_inbox(&platform, &mut app);
    assert!(drawn, "the card asks a question, so the group draws");
    assert!(height > 26.0, "the group holds an item: {height}");
}

/// One frame of the group alone, with the height that it took.
fn render_inbox(platform: &Context, app: &mut HorizonApp) -> (bool, f32) {
    let mut drawn = false;
    let mut height = 0.0;
    let output = platform.run_ui(raw_input(VIEWPORT, None), |ui| {
        let mut actions = SidebarActions::default();
        let top = ui.cursor().top();
        drawn = app.render_sidebar_inbox(ui, &mut actions);
        height = ui.cursor().top() - top;
    });
    let _ = output.discard_textures();
    (drawn, height)
}

#[test]
fn a_frame_that_changed_nothing_reads_no_question_again() {
    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    card_panel(&mut app, workspace, asking_card("Run the plan?", &[("go", "Go")]));
    let start = std::time::Instant::now();
    app.refresh_sidebar_rows(start);
    assert_eq!(app.sidebar_list.questions.len(), 1);
    let read = app.sidebar_list.questions[0].clone();

    app.refresh_sidebar_rows(start + std::time::Duration::from_millis(100));
    assert_eq!(
        app.sidebar_list.refreshed,
        Some(start),
        "a second within the read reads nothing"
    );
    assert_eq!(app.sidebar_list.questions[0], read, "the questions of the cache stay");

    // A new card of an agent, and a press, change the version that the sidebar reads.
    let second = card_panel(
        &mut app,
        workspace,
        asking_card("Push the branch?", &[("push", "Push")]),
    );
    app.settle_block_clicks();
    let soon = start + std::time::Duration::from_millis(200);
    app.refresh_sidebar_rows(soon);
    assert_eq!(app.sidebar_list.refreshed, Some(soon), "the new card is read at once");
    assert_eq!(app.sidebar_list.questions.len(), 2);

    let _ = app.press_card_button(second, "push");
    let later = soon + std::time::Duration::from_millis(200);
    app.refresh_sidebar_rows(later);
    assert_eq!(app.sidebar_list.refreshed, Some(later), "the press is read at once");
    assert_eq!(
        app.sidebar_list.questions.len(),
        1,
        "the answered question left the group"
    );
}

#[test]
fn the_line_of_a_row_says_the_state_of_its_orchestrator_and_the_facts_of_today() {
    use horizon_core::workspace_list::{Orchestrator, Turn, TurnEnd};

    let (_temp, mut app) = test_app();
    let workspace = app.board.create_workspace("Lane");
    let quiet = Turn { busy: false, failed: 0 };
    assert_eq!(Orchestrator::of(0, quiet, None).word(), None);
    assert_eq!(
        Orchestrator::of(0, Turn { busy: true, ..quiet }, None).word(),
        Some("Working")
    );
    assert_eq!(Orchestrator::of(0, quiet, Some(TurnEnd::Done)).word(), Some("Done"));
    assert_eq!(Orchestrator::of(0, quiet, Some(TurnEnd::Failed)).word(), Some("Failed"));

    // A question of a card of the workspace says `Needs you` with the facts of the
    // row of today after it.
    card_panel(&mut app, workspace, asking_card("Run the plan?", &[("go", "Go")]));
    app.refresh_sidebar_rows(std::time::Instant::now());
    let row = app.sidebar_row(workspace);
    assert_eq!(row.line, "Needs you", "the word comes first: {}", row.line);
    assert_eq!(row.attention, horizon_core::Attention::NeedsYou);
}

#[test]
fn the_line_that_names_a_failed_turn_of_the_dock_is_the_line_of_the_conversation() {
    use horizon_core::agent_bridge::{BridgeEvent, Conversation, Entry};

    // The row of a workspace tells a turn that failed from one that ended well by
    // the count of these lines, so the line of the dock is the line that the
    // conversation writes.
    let mut conversation = Conversation::new();
    conversation.apply(BridgeEvent::TurnEnded {
        failed: false,
        duration_ms: None,
        cost_usd: None,
    });
    assert!(
        !conversation.snapshot().entries.iter().any(
            |entry| matches!(entry, Entry::Notice { text } if text == crate::app::orchestrator_dock::TURN_FAILED_LINE)
        ),
        "a turn that ended well writes no line"
    );
    conversation.apply(BridgeEvent::TurnEnded {
        failed: true,
        duration_ms: None,
        cost_usd: None,
    });
    assert_eq!(
        conversation
            .snapshot()
            .entries
            .iter()
            .filter(|entry| matches!(entry, Entry::Notice { text } if text == crate::app::orchestrator_dock::TURN_FAILED_LINE))
            .count(),
        1,
        "the turn that failed is the one line that the dock counts"
    );
}
