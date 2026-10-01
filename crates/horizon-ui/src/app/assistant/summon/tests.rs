use super::*;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};

#[test]
fn the_up_arrow_walks_back_through_what_was_sent_and_down_ends_empty() {
    let mut summon = Summon::default();
    summon.remember("first");
    summon.remember("second");

    summon.recall(true);
    assert_eq!(summon.text, "second");
    summon.recall(true);
    assert_eq!(summon.text, "first");
    summon.recall(true);
    assert_eq!(summon.text, "first", "the oldest line stays put");
    summon.recall(false);
    assert_eq!(summon.text, "second");
    summon.recall(false);
    assert!(summon.text.is_empty());
    assert_eq!(summon.recalled, None);
}

#[test]
fn history_skips_repeats_and_is_bounded() {
    let mut summon = Summon::default();
    summon.remember("same");
    summon.remember("same");
    assert_eq!(summon.history.len(), 1);
    for index in 0..(HISTORY + 10) {
        summon.remember(&format!("line {index}"));
    }
    assert_eq!(summon.history.len(), HISTORY);
    assert_eq!(summon.history.last().map(String::as_str), Some("line 59"));
}

#[test]
fn summoning_opens_the_overlay_and_takes_the_keyboard_then_gives_it_back() {
    let (_temp, mut app) = test_app();
    app.board.focused = Some(horizon_core::PanelId(7));

    app.summon_assistant();
    assert!(app.assistant.summon.is_open());
    assert_eq!(app.board.focused, None);

    app.summon_assistant();
    assert!(!app.assistant.summon.is_open());
}

#[test]
fn the_overlay_starts_the_assistant_even_when_the_drawer_is_closed() {
    let (_temp, mut app) = test_app();
    let ctx = egui::Context::default();
    app.summon_assistant();
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
    run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));

    assert!(!app.assistant.open, "the drawer stays closed");
    assert!(app.board.assistant_panel().is_some());
}

#[test]
fn open_as_chat_carries_the_line_to_the_drawer() {
    let (_temp, mut app) = test_app();
    app.summon_assistant();
    app.assistant.summon.text = "check the cloud".to_string();

    app.open_summon_as_chat();

    assert!(!app.assistant.summon.is_open());
    assert!(app.assistant.open);
    assert_eq!(app.assistant.command.text_for_tests(), "check the cloud");
}

#[test]
fn a_plan_reported_by_the_assistant_is_kept_and_an_empty_one_clears_it() {
    use horizon_core::browser::manifest::agent_panels::{PlanStep, StepStatus};
    let (_temp, mut app) = test_app();
    let step = |title: &str, status| PlanStep {
        title: title.to_string(),
        detail: None,
        status,
    };

    app.assistant_set_plan(vec![step("one", StepStatus::Done), step("two", StepStatus::Running)]);
    assert_eq!(super::plan::progress(&app.assistant.plan), "1 of 2 done");

    app.assistant_set_plan(Vec::new());
    assert!(app.assistant.plan.is_empty());
}

#[test]
fn the_canvas_leaves_the_pointer_alone_under_the_overlay() {
    let (_temp, mut app) = test_app();
    let inside = egui::pos2(500.0, 500.0);
    assert!(!app.assistant_summon_covers(inside), "nothing covers it while closed");

    app.summon_assistant();
    app.assistant.summon.rect = Some(egui::Rect::from_min_max(
        egui::pos2(400.0, 400.0),
        egui::pos2(900.0, 600.0),
    ));
    assert!(app.assistant_summon_covers(inside));
    assert!(!app.assistant_summon_covers(egui::pos2(100.0, 100.0)));

    app.summon_assistant();
    assert!(!app.assistant_summon_covers(inside), "closing forgets the rectangle");
}
