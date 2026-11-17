//! A finished card folds to one line, and the person folds and unfolds any card.

use egui::{Event, Pos2, Vec2};
use horizon_core::{CanvasViewState, WindowConfig};

use super::columns::{AGENT, GAP, WINDOW, agent_board, place, pointer, settle, started};
use super::*;
use crate::app::test_support::{card_panel, drawn_text, drew_text};

/// The height of a card that is drawn as one line: the title bar of a panel.
const LINE: f32 = 34.0;
/// The words of the body of the card of these lanes.
const BODY: &str = "The server answers on its port.";

/// The card of a job, with a heading, a badge, a body and one button.
fn job(heading: &str, badge: &str, finished: Option<bool>) -> serde_json::Value {
    let mut document = serde_json::json!({"components": [
        {"type": "heading", "text": heading, "badge": {"text": badge, "tone": "ok"}},
        {"type": "text", "text": BODY},
        {"type": "buttons", "items": [{"id": "again", "label": "Run again", "tone": "primary"}]}
    ]});
    if let Some(finished) = finished {
        document["finished"] = serde_json::Value::Bool(finished);
    }
    document
}

/// Draws one card of the agent panel and returns the frame that shows it.
fn draw(ctx: &Context, app: &mut HorizonApp, document: serde_json::Value) -> (String, egui::FullOutput) {
    let (card, _) = created(app.apply_block_request(
        &request(
            app,
            Operation::Create {
                title: Some("the job".into()),
                document,
            },
        ),
        ctx,
    ));
    (card, frame(ctx, app))
}

/// Sends a new document of the card `card` and returns the frame that shows it.
fn update(ctx: &Context, app: &mut HorizonApp, card: &str, document: serde_json::Value) -> egui::FullOutput {
    let operation = Operation::Update {
        panel_id: card.to_owned(),
        document,
    };
    let updated = app.apply_block_request(&request(app, operation), ctx);
    assert!(
        matches!(updated, Outcome::Updated { .. }),
        "the agent updates its card: {updated:?}"
    );
    frame(ctx, app)
}

/// The frames that settle the board, and the frame after them.
fn frame(ctx: &Context, app: &mut HorizonApp) -> egui::FullOutput {
    settle(ctx, app);
    run_app_frame_with_input(ctx, app, raw_input(WINDOW, None))
}

/// One press of the person at `position` of the window, and the frame after it.
fn press(ctx: &Context, app: &mut HorizonApp, position: Pos2) -> egui::FullOutput {
    for events in [
        vec![Event::PointerMoved(position)],
        vec![pointer(position, true)],
        vec![pointer(position, false)],
    ] {
        let mut input = raw_input(WINDOW, None);
        input.events = events;
        let _ = run_app_frame_with_input(ctx, app, input);
    }
    frame(ctx, app)
}

/// One press of the person on the title of the card `card`, on its one line.
fn press_line(ctx: &Context, app: &mut HorizonApp, card: &str) -> egui::FullOutput {
    let line = card_panel(app, ctx, card).min + Vec2::new(60.0, 17.0) * app.canvas_view.zoom;
    press(ctx, app, line)
}

/// One press of the person on the fold control of the card `card`, which stands
/// left of its close control.
fn press_fold(ctx: &Context, app: &mut HorizonApp, card: &str) -> egui::FullOutput {
    let panel = card_panel(app, ctx, card);
    let control = Pos2::new(panel.max.x, panel.min.y) + Vec2::new(-44.0, 17.0) * app.canvas_view.zoom;
    press(ctx, app, control)
}

fn height(app: &HorizonApp, card: &str) -> f32 {
    place(app, card)[3] - place(app, card)[1]
}

fn folded(app: &HorizonApp, card: &str) -> bool {
    (height(app, card) - LINE).abs() < 0.5
}

#[test]
fn a_finished_card_is_one_line_that_holds_its_heading_and_its_badge() {
    let (_temp, ctx, mut app) = agent_board(None);
    let other = serde_json::json!({"components": [{"type": "heading", "text": "Bring the folder"}]});
    let (under, _) = draw(&ctx, &mut app, other);

    let (card, output) = draw(&ctx, &mut app, job("Server ready", "Done", Some(true)));

    let rect = place(&app, &card);
    assert!(
        folded(&app, &card) && (rect[2] - rect[0] - 420.0).abs() < 0.5,
        "a finished card is one line at the width of its column: {rect:?}"
    );
    let panel = card_panel(&app, &ctx, &card);
    for words in ["Server ready", "Done"] {
        let ink = drawn_text(&output, words);
        assert!(panel.contains_rect(ink), "the line holds {words}: {ink:?} in {panel:?}");
    }
    assert!(!drew_text(&output, "Run again"), "a folded card draws no body");
    assert!(
        (place(&app, &under)[1] - (rect[3] + GAP)).abs() < 0.5,
        "the card under the line stands directly under it: {:?} under {rect:?}",
        place(&app, &under)
    );
    assert!(!folded(&app, &under), "a card without the field is not folded");
}

#[test]
fn a_press_on_the_line_unfolds_the_card_and_a_press_on_its_control_folds_it_again() {
    let (_temp, ctx, mut app) = agent_board(None);
    let (card, _) = draw(&ctx, &mut app, job("Server ready", "Done", Some(true)));

    let output = press_line(&ctx, &mut app, &card);
    assert!(
        !folded(&app, &card) && drew_text(&output, BODY),
        "a press on the line unfolds the card: {}",
        height(&app, &card)
    );

    // The card works as every card: a press on its button is a click for its agent.
    let _ = press(&ctx, &mut app, drawn_text(&output, "Run again").center());
    let listed = app.apply_block_request(&request(&app, Operation::List), &ctx);
    let Outcome::Panels { panels } = listed else {
        panic!("the list of the cards: {listed:?}");
    };
    assert_eq!(
        panels[0].pending_clicks, 1,
        "the button of the unfolded card takes a press"
    );

    // A later document of the finished job leaves the card as the person left it.
    let _ = update(&ctx, &mut app, &card, job("Server ready", "Done", Some(true)));
    assert!(!folded(&app, &card), "an update folds no card that the person unfolded");

    let output = press_fold(&ctx, &mut app, &card);
    assert!(
        folded(&app, &card) && !drew_text(&output, BODY),
        "a press on the control folds the card again: {}",
        height(&app, &card)
    );
    let _ = press_fold(&ctx, &mut app, &card);
    assert!(!folded(&app, &card), "the same control unfolds it");
}

#[test]
fn an_update_folds_a_card_only_when_it_says_that_the_job_is_finished() {
    let (_temp, ctx, mut app) = agent_board(None);
    let (card, _) = draw(&ctx, &mut app, job("Start the server", "Working", None));
    assert!(!folded(&app, &card), "a card without the field opens unfolded");

    let _ = update(&ctx, &mut app, &card, job("Start the server", "Still working", None));
    let _ = update(&ctx, &mut app, &card, job("Start the server", "Working", Some(false)));
    assert!(!folded(&app, &card), "an update folds no card that is not finished");

    let output = update(&ctx, &mut app, &card, job("Server ready", "Done", Some(true)));
    assert!(
        folded(&app, &card) && drew_text(&output, "Server ready") && !drew_text(&output, BODY),
        "the update that finishes the job folds its card: {}",
        height(&app, &card)
    );

    let output = update(&ctx, &mut app, &card, job("Start the server", "Working", None));
    assert!(
        !folded(&app, &card) && drew_text(&output, BODY),
        "a job that runs again opens its card: {}",
        height(&app, &card)
    );
}

#[test]
fn the_fold_of_the_person_survives_an_update_and_a_restore() {
    let (_temp, ctx, mut app) = agent_board(None);
    let (card, _) = draw(&ctx, &mut app, job("Start the server", "Working", None));

    let _ = press_fold(&ctx, &mut app, &card);
    assert!(folded(&app, &card), "the person folds a card that is not finished");
    let output = update(&ctx, &mut app, &card, job("Start the server", "Still working", None));
    assert!(
        folded(&app, &card) && drew_text(&output, "Still working"),
        "the card stays as the person left it, and its line shows the new badge"
    );

    let mut saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    // The saved agent panel starts no agent program in a lane.
    saved.workspaces[0].panels[0].kind = PanelKind::Editor;
    let (_restored_temp, ctx, mut app) = started(saved);
    let output = frame(&ctx, &mut app);

    assert!(
        folded(&app, &card) && drew_text(&output, "Still working") && !drew_text(&output, BODY),
        "a restore keeps the fold of the person: {}",
        height(&app, &card)
    );
    assert!(
        place(&app, AGENT)[2] < place(&app, &card)[0],
        "the line stands in its column"
    );
    let output = press_fold(&ctx, &mut app, &card);
    assert!(
        !folded(&app, &card) && drew_text(&output, BODY),
        "the restored card unfolds: {}",
        height(&app, &card)
    );
}

/// The colours of the pill that holds the words `words` in `output`: its ink,
/// its fill and its outline.
fn pill(output: &egui::FullOutput, words: &str) -> [egui::Color32; 3] {
    use egui::epaint::Shape;
    let ink = drawn_text(output, words);
    let colour = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == words => {
                text.galley.job.sections.first().map(|section| section.format.color)
            }
            _ => None,
        })
        .expect("the ink of the words");
    // The pill is the smallest painted rectangle around the words.
    let around = |outline: bool| {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Rect(rect) if rect.rect.contains_rect(ink) => Some(rect),
                _ => None,
            })
            .filter(|rect| outline == (rect.stroke.width > 0.0))
            .min_by(|left, right| left.rect.area().total_cmp(&right.rect.area()))
            .expect("the pill of the words")
    };
    [colour, around(false).fill, around(true).stroke.color]
}

#[test]
fn the_badge_of_a_folded_card_keeps_the_colours_of_its_tone_in_both_themes() {
    for light in [false, true] {
        let (_temp, ctx, mut app) = agent_board(None);
        let _theme = light.then(|| {
            let mut config = app.template_config.clone();
            config.appearance.theme = horizon_core::AppearanceTheme::Light;
            app.apply_runtime_config(&config);
            crate::test_egui::with_theme(crate::theme::ResolvedTheme::Light)
        });
        // The frames that apply the theme. After them, no fade scales a colour: a
        // card fades in, and this lane judges the colours themselves.
        settle(&ctx, &mut app);
        ctx.global_style_mut(|style| style.animation_time = 0.0);

        let mut pills = Vec::new();
        for (words, tone) in [("Failed", "danger"), ("Done", "ok"), ("Waiting", "warn")] {
            let mut document = job("The server", words, None);
            document["components"][0]["badge"]["tone"] = tone.into();
            let (card, open) = draw(&ctx, &mut app, document);
            let open_pill = pill(&open, words);

            let line = press_fold(&ctx, &mut app, &card);
            assert!(folded(&app, &card), "the person folds the card of {words}");
            assert_eq!(
                pill(&line, words),
                open_pill,
                "the folded line paints the badge {words} in the colours of the open card (light {light})"
            );
            pills.push(open_pill);
        }
        assert!(
            pills[0] != pills[1] && pills[1] != pills[2] && pills[0] != pills[2],
            "each tone has its own colours (light {light}): {pills:?}"
        );
    }
}
