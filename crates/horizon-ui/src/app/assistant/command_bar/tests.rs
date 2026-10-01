use super::*;

fn names(text: &str, agent: PanelKind) -> Vec<&'static str> {
    suggestions(text, agent).iter().map(|command| command.name).collect()
}

#[test]
fn plain_text_is_a_message_and_blank_is_nothing() {
    assert_eq!(
        parse("  check the cloud  "),
        Entry::Message("check the cloud".to_string())
    );
    assert_eq!(parse("   "), Entry::Nothing);
    assert_eq!(parse(""), Entry::Nothing);
}

#[test]
fn horizon_commands_run_locally_and_everything_else_is_forwarded() {
    assert_eq!(parse("/new"), Entry::Local(LocalCommand::NewThread));
    assert_eq!(parse("/ask"), Entry::Local(LocalCommand::ToggleAsk));
    assert_eq!(parse("/compact"), Entry::Forward("/compact".to_string()));
    assert_eq!(
        parse("/never-heard-of-it"),
        Entry::Forward("/never-heard-of-it".to_string())
    );
    // A Horizon command with an argument belongs to the agent.
    assert_eq!(parse("/new thing"), Entry::Forward("/new thing".to_string()));
}

#[test]
fn control_characters_never_reach_the_agent() {
    assert_eq!(parse("hello\u{1b}[31m"), Entry::Message("hello[31m".to_string()));
    assert_eq!(parse("\u{3}\u{4}"), Entry::Nothing);
}

#[test]
fn a_bare_slash_lists_horizon_commands_then_the_agents() {
    let listed = names("/", PanelKind::Claude);
    assert_eq!(&listed[..4], ["new", "threads", "engine", "ask"]);
    assert!(listed.contains(&"compact"));
    assert!(listed.len() <= MAX_SUGGESTIONS);
}

#[test]
fn prefix_matches_come_before_substring_matches() {
    let listed = names("/mo", PanelKind::Claude);
    assert_eq!(listed, ["model"]);
    let listed = names("/s", PanelKind::Claude);
    assert_eq!(listed[0], "status");
    assert!(listed.contains(&"resume"));
}

#[test]
fn the_list_follows_the_agent_and_stops_at_an_argument() {
    assert!(names("/", PanelKind::Codex).contains(&"diff"));
    assert!(!names("/", PanelKind::Claude).contains(&"diff"));
    assert!(names("/model gpt", PanelKind::Codex).is_empty());
    assert!(names("model", PanelKind::Codex).is_empty());
}

#[test]
fn every_agent_offers_help_and_no_name_repeats() {
    for agent in horizon_core::assistant::ASSISTANT_AGENTS {
        let listed = names("/", agent);
        assert!(listed.contains(&"help"), "{agent:?}");
        let mut unique = listed.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), listed.len(), "{agent:?}");
    }
}

mod in_the_app {
    use egui::Context;

    use super::super::*;
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};

    fn running_assistant() -> (tempfile::TempDir, Context, HorizonApp) {
        let (temp, mut app) = test_app();
        let ctx = Context::default();
        app.toggle_assistant();
        run_app_frame_with_input(&ctx, &mut app, raw_input([1400.0, 900.0], None));
        assert!(app.board.assistant_panel().is_some());
        (temp, ctx, app)
    }

    #[test]
    fn a_message_is_typed_into_the_assistant_and_the_field_clears() {
        let (_temp, _ctx, mut app) = running_assistant();
        let panel = app.board.assistant_panel().expect("assistant");
        app.assistant.command.text = "what is running?".to_string();

        app.run_command(&parse("what is running?"));

        assert!(app.assistant.command.text.is_empty());
        assert!(app.agent_panel_requests.in_flight(panel, Instant::now()));
    }

    #[test]
    fn a_message_without_an_assistant_keeps_the_text_and_says_why() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.assistant.command.text = "hello".to_string();

        app.run_command(&parse("hello"));

        assert_eq!(app.assistant.command.text, "hello");
        assert!(app.assistant.command.feedback.is_some());
    }

    #[test]
    fn the_ask_command_flips_approval_and_remembers_it() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let before = app.assistant.settings.ask_before_send;

        app.run_command(&parse("/ask"));

        assert_eq!(app.assistant.settings.ask_before_send, !before);
        assert_eq!(
            horizon_core::assistant::AssistantSettings::load(&app.assistant.home).ask_before_send,
            !before
        );
        assert!(app.assistant.command.feedback.is_some());
    }

    #[test]
    fn the_engine_and_threads_commands_open_their_popups() {
        let (_temp, mut app) = crate::app::test_support::test_app();

        app.run_command(&parse("/engine"));
        app.run_command(&parse("/threads"));

        assert!(app.assistant.engine_open);
        assert!(app.assistant.thread_menu_open);
    }

    #[test]
    fn the_new_command_restarts_the_assistant_in_a_fresh_thread() {
        let (_temp, _ctx, mut app) = running_assistant();
        app.assistant.active_session = Some("session".to_string());

        app.run_command(&parse("/new"));

        assert!(app.assistant.restart_requested);
        assert_eq!(app.assistant.active_session, None);
    }

    #[test]
    fn revealing_the_assistant_opens_its_drawer_instead_of_panning_the_canvas() {
        let (_temp, ctx, mut app) = running_assistant();
        let panel = app.board.assistant_panel().expect("assistant");
        app.toggle_assistant();
        assert!(!app.assistant.open);

        app.reveal_selected_panel(&ctx, panel);

        assert!(app.assistant.open);
    }

    #[test]
    fn a_replaced_board_drops_cards_focus_and_pending_resume() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        app.assistant
            .cards
            .push(crate::app::assistant::cards::CardKind::Declined {
                title: "codex".to_string(),
                reason: "gone".to_string(),
            });
        app.assistant.command.text = "draft".to_string();
        app.assistant.focused = true;
        app.assistant.active_session = Some("old".to_string());

        app.assistant.reset_for_new_board();

        assert!(app.assistant.cards.is_empty());
        assert!(app.assistant.command.text.is_empty());
        assert!(!app.assistant.focused);
        assert_eq!(app.assistant.active_session, None);
    }
}
