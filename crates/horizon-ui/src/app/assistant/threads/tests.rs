use egui::Context;
use horizon_core::PanelKind;
use horizon_core::assistant::Thread;

use super::*;
use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};

fn frame(ctx: &Context, app: &mut HorizonApp) {
    run_app_frame_with_input(ctx, app, raw_input([1400.0, 900.0], None));
}

fn remembered(session: &str, agent: PanelKind) -> Thread {
    Thread {
        session_id: session.to_string(),
        agent,
        title: "Earlier work".to_string(),
        space: "work".to_string(),
        cwd: Some("/work".to_string()),
        updated_at: 1,
    }
}

#[test]
fn the_running_session_becomes_a_thread_in_its_workspace() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.toggle_assistant();
    frame(&ctx, &mut app);
    // The sync is rate limited; let it run on the frame after the agent exists.
    app.assistant.last_thread_sync = None;
    frame(&ctx, &mut app);

    let session = app
        .assistant
        .active_session
        .clone()
        .expect("Claude is bound to a session at launch");
    let thread = app.assistant.threads.get(&session).expect("thread recorded");
    assert_eq!(thread.agent, PanelKind::Claude);
    assert!(!thread.space.is_empty(), "the thread names the workspace it ran in");
}

#[test]
fn switching_resumes_the_chosen_session_with_its_agent() {
    let (_temp, mut app) = test_app();
    let ctx = Context::default();
    app.toggle_assistant();
    frame(&ctx, &mut app);
    let first = app.board.assistant_panel().expect("assistant started");
    app.assistant
        .threads
        .upsert(remembered("codex-session", PanelKind::Codex));

    app.switch_assistant_thread("codex-session");
    assert_eq!(
        app.assistant.settings.agent,
        PanelKind::Codex,
        "the thread's agent takes over"
    );
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);

    let second = app.board.assistant_panel().expect("assistant restarted");
    assert_ne!(first, second);
    let panel = app.board.panel(second).expect("panel");
    assert_eq!(panel.kind, PanelKind::Codex);
    assert!(matches!(
        &panel.resume,
        horizon_core::PanelResume::Session { session_id } if session_id == "codex-session"
    ));
    assert!(app.assistant.resume.is_none(), "the resume request is used once");
}

#[test]
fn switching_to_the_session_already_running_changes_nothing() {
    let (_temp, mut app) = test_app();
    app.assistant.threads.upsert(remembered("same", PanelKind::Claude));
    app.assistant.active_session = Some("same".to_string());
    app.switch_assistant_thread("same");
    assert!(!app.assistant.restart_requested);
    assert!(app.assistant.resume.is_none());
}

#[test]
fn a_new_thread_starts_fresh_and_keeps_the_old_one_listed() {
    let (_temp, mut app) = test_app();
    app.assistant.threads.upsert(remembered("old", PanelKind::Claude));
    app.assistant.active_session = Some("old".to_string());
    app.assistant.resume = Some(remembered("old", PanelKind::Claude));

    app.new_assistant_thread();

    assert!(app.assistant.restart_requested);
    assert!(app.assistant.resume.is_none());
    assert!(app.assistant.active_session.is_none());
    assert!(app.assistant.threads.get("old").is_some());
}

#[test]
fn long_titles_are_shortened_for_the_bar() {
    assert_eq!(shortened("short"), "short");
    let long = "x".repeat(80);
    let shown = shortened(&long);
    assert_eq!(shown.chars().count(), 38);
    assert!(shown.ends_with('…'));
}
