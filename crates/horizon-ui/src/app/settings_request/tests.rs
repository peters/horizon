//! The two settings that an agent asks for and only the person confirms, judged
//! on a test board.

use egui::Context;
use horizon_browser_control::block::validate_document;
use horizon_browser_control::manifest::block::{
    self, BrowserEngine, HOST_APPLY_TIMEOUT, Operation, Outcome, Request, SETTING_REQUEST_TIMEOUT_MILLIS,
};
use horizon_core::browser::manifest;
use horizon_core::{BrowserHostConfig, Config, PanelKind, RuntimeState, StartupDecision};

use super::*;
use crate::app::test_support::{browser_panel_options, editor_workspace_state, test_app_with_startup};

type TestApp = (tempfile::TempDir, Context, HorizonApp);

fn app() -> TestApp {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state("first", [0.0, 0.0]),
            editor_workspace_state("second", [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    // A link that this binary starts runs `/bin/true` instead of `ssh`, so a test
    // of a confirmed server entry starts no `ssh` process.
    app.link_child = Some("/bin/true".to_owned());
    // The store of this home holds the settings that the test app started with,
    // so a read of it answers before the first confirmed setting.
    let config = app.template_config.clone();
    app.settings_io.save_config(app.config_path.clone(), config, Vec::new());
    app.wait_for_settings_io();
    (temp, ctx, app)
}

fn request(app: &HorizonApp, operation: Operation) -> Request {
    Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[0].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis()
            + i64::try_from(HOST_APPLY_TIMEOUT.as_millis()).unwrap_or(0)
            + i64::try_from(SETTING_REQUEST_TIMEOUT_MILLIS).unwrap_or(0),
        operation,
    }
}

fn link(ssh_target: &str) -> Operation {
    Operation::ServerLinkRequest {
        ssh_target: Some(ssh_target.to_owned()),
        machine: None,
        command: Some("/opt/horizon --server-link".to_owned()),
    }
}

fn link_machine(machine: &str) -> Operation {
    Operation::ServerLinkRequest {
        ssh_target: None,
        machine: Some(machine.to_owned()),
        command: Some("/opt/horizon --server-link".to_owned()),
    }
}

/// Runs the parts of a frame that apply the answer of a link, so a confirmed
/// server entry answers once its link knows whether the server is there.
fn settle_link(app: &mut HorizonApp) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        app.poll_machine_load();
        app.poll_server_sync();
        app.settle_browser_host_io();
        if !app.settings_request.is_waiting() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the link of the confirmed entry answered in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn browser(ssh_target: &str, profile: &str, engine: Option<BrowserEngine>) -> Operation {
    Operation::BrowserHostRequest {
        ssh_target: Some(ssh_target.to_owned()),
        machine: None,
        profile: profile.to_owned(),
        engine,
    }
}

/// A browser host of a machine of the machine list, which answers to the machine
/// key of Horizon alone.
fn browser_on_machine(machine: &str, profile: &str) -> Operation {
    Operation::BrowserHostRequest {
        ssh_target: None,
        machine: Some(machine.to_owned()),
        profile: profile.to_owned(),
        engine: None,
    }
}

/// The card that a setting request drew, in the caller's workspace.
fn asking_card(app: &mut HorizonApp) -> String {
    let waiting = app
        .settings_request
        .waiting
        .as_ref()
        .expect("the request holds a card open");
    let card = app.board.panel(waiting.panel).expect("the card is on the board");
    card.block().expect("the card is a card");
    card.local_id.clone()
}

/// The words of the card that asks the person.
fn card_text(app: &mut HorizonApp, card: &str) -> String {
    let id = app.board.panel_id_by_local_id(card).expect("the card of the request");
    let document = app
        .board
        .panel(id)
        .and_then(|panel| panel.block())
        .map(|block| block.document.clone())
        .expect("the document of the card");
    format!("{document:?}")
}

fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

fn message(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { message, .. } => message,
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// The settings that the store of this test holds now, after the settings worker
/// wrote what the person confirmed.
fn stored(app: &mut HorizonApp) -> Config {
    app.wait_for_settings_io();
    Config::load(Some(&app.config_path)).expect("the store of this test")
}

/// The answer that the host wrote for one request, as the tool reports it.
fn answer_at(root: &std::path::Path, request: &Request) -> serde_json::Value {
    let outcome = block::take_result_at(root, request)
        .expect("the answer is readable")
        .expect("the host answered the request");
    serde_json::to_value(outcome).expect("the answer as JSON")
}

#[test]
fn a_server_link_draws_a_card_of_horizon_that_states_the_entry() {
    let (_temp, ctx, mut app) = app();
    let outcome = app.apply_block_request_waiting(&request(&app, link("server-vm")), &ctx);
    assert!(outcome.is_none(), "the request waits for the person");

    let panel = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&panel).expect("the card of the request");
    assert!(
        app.card_host_owned(id),
        "Horizon wrote this document, so its buttons are the host's controls"
    );
    let document = app
        .board
        .panel(id)
        .and_then(|panel| panel.block())
        .map(|block| block.document.clone())
        .expect("the document of the card");
    assert_eq!(validate_document(&document), Ok(()), "the card is a valid card");
    let text = format!("{document:?}");
    for word in [
        "Link this computer to the Horizon server on server-vm?",
        SERVER_SENT,
        SERVER_KEPT,
        "Link",
        "Not now",
    ] {
        assert!(text.contains(word), "the card says {word}: {text}");
    }
    assert!(
        text.contains("/opt/horizon --server-link"),
        "the card shows the command as the store holds it: {text}"
    );
    assert_eq!(
        document.components.len(),
        5,
        "the heading, the two stored values, the two statements and the buttons"
    );
}

#[test]
fn a_press_on_link_writes_the_store_and_starts_the_link_without_a_restart() {
    let (_temp, ctx, mut app) = app();
    let before = stored(&mut app).server;
    assert!(
        app.apply_block_request_waiting(&request(&app, link("server-vm")), &ctx)
            .is_none()
    );
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");

    app.press_host_card_button(&ctx, id, APPLY);

    let answer = app
        .board
        .panel(id)
        .map_or("the card went with the answer", |_| "the card stayed");
    assert_eq!(answer, "the card went with the answer");
    let after = stored(&mut app);
    assert_eq!(after.server.ssh_target.as_deref(), Some("server-vm"));
    assert_eq!(after.server.command.as_deref(), Some("/opt/horizon --server-link"));
    assert!(!before.names_server(), "the store named no server before the press");
    assert!(
        app.template_config.server.names_server(),
        "the link of this run reads the entry that the person confirmed"
    );
}

/// A pressed `Link` answers `confirmed` and `linked` only after the link of the new
/// entry says whether the server answered: the stand-in child of this test ends at
/// once, so the server is away and the answer carries the reason of the attempt in
/// words.
#[test]
fn the_answer_of_a_pressed_link_is_confirmed_with_the_state_of_its_link() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let request = request(&app, link("server-vm"));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    assert!(
        app.settings_request.is_waiting(),
        "the answer waits for the first answer of the server"
    );
    settle_link(&mut app);

    let answer = answer_at(&root, &request);
    assert_eq!(answer["status"], serde_json::json!("server_link"));
    assert_eq!(answer["answer"], serde_json::json!("confirmed"));
    assert_eq!(
        answer["start"],
        serde_json::json!("linked"),
        "the entry applies to the running program, so no start is needed"
    );
    assert_eq!(
        answer["link"],
        serde_json::json!("away"),
        "the child of the link ended without a hello: {answer}"
    );
    assert!(
        answer["reason"]
            .as_str()
            .is_some_and(|reason| !reason.trim().is_empty()),
        "the reason of the link reaches the agent in words: {answer}"
    );
}

/// A card that names a machine of the list states the machine and the login that
/// the link uses, and the entry stores the name of the machine.
#[test]
fn a_card_that_names_a_machine_states_it_and_stores_its_name() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let request = request(&app, link_machine("rented-1"));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    let document = app
        .board
        .panel(id)
        .and_then(|panel| panel.block())
        .map(|block| block.document.clone())
        .expect("the document of the card");
    assert_eq!(validate_document(&document), Ok(()), "the card is a valid card");
    let text = format!("{document:?}");
    for word in [
        "Link this computer to the Horizon server on the machine rented-1?",
        SERVER_MACHINE,
        SERVER_SENT,
    ] {
        assert!(text.contains(word), "the card says {word}: {text}");
    }
    assert!(!text.contains("Server target"), "a machine names no raw target: {text}");

    app.press_host_card_button(&ctx, id, APPLY);
    settle_link(&mut app);
    let stored = stored(&mut app);
    assert_eq!(stored.server.machine.as_deref(), Some("rented-1"));
    assert_eq!(
        stored.server.ssh_target, None,
        "the entry names the machine alone, so a new address of it needs no edit"
    );
}

/// A card names the whole entry of the server, so an address that a join of another
/// server wrote does not survive the person's press: the link would keep trying that
/// listener instead of the login of the entry.
#[test]
fn a_confirmed_entry_replaces_the_address_of_a_join() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let mut joined = app.template_config.clone();
    joined.server.address = Some("100.64.0.7:7000".to_owned());
    app.template_config.server = joined.server.clone();
    app.settings_io.save_config(app.config_path.clone(), joined, Vec::new());
    app.wait_for_settings_io();

    let request = request(&app, link("server-vm"));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    settle_link(&mut app);

    let stored = stored(&mut app);
    assert_eq!(stored.server.ssh_target.as_deref(), Some("server-vm"));
    assert_eq!(
        stored.server.address, None,
        "the entry of the card names the route, so the listener of another server goes"
    );
}

#[test]
fn a_press_on_not_now_leaves_the_settings_as_they_are() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let request = request(&app, link("server-vm"));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, DECLINE);
    app.settle_browser_host_io();

    assert_eq!(
        answer_at(&root, &request),
        serde_json::json!({"status": "server_link", "answer": "declined"}),
        "a person who pressed Not now starts nothing"
    );
    let after = stored(&mut app);
    assert!(!after.server.names_server(), "Not now writes no entry: {after:?}");
    assert!(!app.template_config.server.names_server());
}

#[test]
fn no_press_writes_nothing() {
    let (_temp, ctx, mut app) = app();
    assert!(
        app.apply_block_request_waiting(&request(&app, link("server-vm")), &ctx)
            .is_none()
    );
    let second = app.apply_block_request(&request(&app, browser("browser-vm", "work", None)), &ctx);
    assert_eq!(code(&second), REQUEST_PENDING, "one question at a time");
    let after = stored(&mut app);
    assert!(!after.server.names_server(), "the card changed no setting: {after:?}");
    assert_eq!(after.browser_host, BrowserHostConfig::default(), "and no browser host");
    assert!(
        app.template_config.server.ssh_target.is_none(),
        "the run holds the setting of its start"
    );
}

#[test]
fn a_second_request_while_one_waits_is_refused_with_words() {
    let (_temp, ctx, mut app) = app();
    assert!(
        app.apply_block_request_waiting(&request(&app, link("server-vm")), &ctx)
            .is_none()
    );
    let outcome = app.apply_block_request(&request(&app, browser("browser-vm", "work", None)), &ctx);
    assert_eq!(code(&outcome), REQUEST_PENDING);
    assert!(
        message(&outcome).contains("already asks the person"),
        "the refusal has words: {outcome:?}"
    );
    let cards = app.workspace_cards(app.board.panels[0].workspace_id).len();
    assert_eq!(cards, app.workspace_cards(app.board.panels[0].workspace_id).len());
    let _ = cards;
    assert_eq!(
        app.workspace_cards(app.board.panels[0].workspace_id)
            .iter()
            .filter(|panel| panel.block().is_some_and(|card| card.owner_actor.is_none()))
            .count(),
        1,
        "the second request drew no second card"
    );
}

#[test]
fn the_card_of_a_browser_host_states_the_profile_and_the_engine() {
    let (_temp, ctx, mut app) = app();
    assert!(
        app.apply_block_request_waiting(&request(&app, browser("browser-vm", "work", None)), &ctx)
            .is_none()
    );
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    let document = app
        .board
        .panel(id)
        .and_then(|panel| panel.block())
        .map(|block| block.document.clone())
        .expect("the document of the card");
    assert_eq!(validate_document(&document), Ok(()));
    let text = format!("{document:?}");
    for word in [
        "Use the browser on browser-vm for new browser panels?",
        "Chromium",
        "work",
        BROWSER_USES,
        BROWSER_APPLIES,
        "Use it",
        "Not now",
    ] {
        assert!(text.contains(word), "the card says {word}: {text}");
    }
}

#[test]
fn a_press_on_use_it_writes_the_browser_host_of_this_computer() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let request = request(&app, browser("browser-vm", "work", Some(BrowserEngine::Chromium)));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    app.settle_browser_host_io();

    assert_eq!(
        answer_at(&root, &request),
        serde_json::json!({"status": "browser_host", "answer": "confirmed"})
    );
    let after = stored(&mut app);
    assert_eq!(after.browser_host.ssh_target.as_deref(), Some("browser-vm"));
    assert_eq!(after.browser_host.profile.as_deref(), Some("work"));
    assert!(!after.server.names_server(), "the browser card writes no server entry");
}

/// A browser host on a machine of the list states the machine and the key that the
/// connect uses, and it stores the name of the machine and no SSH target.
#[test]
fn a_browser_host_on_a_machine_of_the_list_states_the_machine_and_stores_its_name() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    // An entry of an earlier press names an SSH target, which this card replaces.
    let mut before = stored(&mut app);
    before.browser_host.ssh_target = Some("browser-vm".to_owned());
    before.browser_host.profile = Some("work".to_owned());
    app.settings_io.save_config(app.config_path.clone(), before, Vec::new());
    app.wait_for_settings_io();
    let request = request(&app, browser_on_machine("rented-1", "work"));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let text = card_text(&mut app, &card);
    for word in [
        "Use the browser on the machine rented-1 for new browser panels?",
        "Machine",
        "rented-1",
        "the machine key of this computer",
        "work",
    ] {
        assert!(text.contains(word), "the card says {word}: {text}");
    }
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    app.settle_browser_host_io();

    let after = stored(&mut app);
    assert_eq!(
        after.browser_host.machine.as_deref(),
        Some("rented-1"),
        "the card stores the machine of the list"
    );
    assert_eq!(
        after.browser_host.ssh_target, None,
        "the card names one route, so the target of the earlier entry does not survive it"
    );
    assert_eq!(after.browser_host.profile.as_deref(), Some("work"));
    assert!(
        after.browser_host.validate().is_ok(),
        "the stored entry passes the rule of the settings: {:?}",
        after.browser_host.validate()
    );
}

#[test]
fn a_browser_host_request_that_names_two_hosts_or_none_draws_no_card() {
    let (_temp, ctx, mut app) = app();
    let none = Operation::BrowserHostRequest {
        ssh_target: None,
        machine: None,
        profile: "work".to_owned(),
        engine: None,
    };
    let outcome = app.apply_block_request(&request(&app, none), &ctx);
    assert_eq!(code(&outcome), block_machines::INVALID_REQUEST);
    assert!(message(&outcome).contains("names no host"), "{outcome:?}");

    let both = Operation::BrowserHostRequest {
        ssh_target: Some("browser-vm".to_owned()),
        machine: Some("rented-1".to_owned()),
        profile: "work".to_owned(),
        engine: None,
    };
    let outcome = app.apply_block_request(&request(&app, both), &ctx);
    assert_eq!(code(&outcome), block_machines::INVALID_REQUEST);
    assert!(message(&outcome).contains("names one host"), "{outcome:?}");
    assert!(
        app.settings_request.waiting.is_none(),
        "a request that names its host twice draws no card"
    );
}

#[test]
fn firefox_on_another_machine_asks_the_person_and_stores_the_engine() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let request = request(&app, browser("browser-vm", "work", Some(BrowserEngine::Firefox)));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let text = card_text(&mut app, &card);
    assert!(
        text.contains("Firefox, with Horizon on that machine"),
        "the card names the engine and what that machine needs: {text}"
    );
    assert!(
        text.contains("geckodriver") && text.contains("horizon binary"),
        "the card says what a Firefox host needs: {text}"
    );
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    app.settle_browser_host_io();
    let after = stored(&mut app);
    assert_eq!(after.browser_host.ssh_target.as_deref(), Some("browser-vm"));
    assert_eq!(after.browser_host.profile.as_deref(), Some("work"));
    assert_eq!(
        after.browser_host.engine,
        horizon_core::BrowserHostEngine::Firefox,
        "the engine of the person's answer is written with the entry"
    );
}

#[test]
fn a_target_that_the_settings_would_refuse_draws_no_card() {
    let (_temp, ctx, mut app) = app();
    for operation in [
        link("-oProxyCommand=evil"),
        link("two hosts"),
        Operation::ServerLinkRequest {
            ssh_target: Some("server-vm".to_owned()),
            machine: None,
            command: Some("horizon; rm -rf /".to_owned()),
        },
        Operation::ServerLinkRequest {
            ssh_target: Some("server-vm".to_owned()),
            machine: Some("rented-1".to_owned()),
            command: None,
        },
        Operation::ServerLinkRequest {
            ssh_target: None,
            machine: None,
            command: None,
        },
        browser("two hosts", "work", None),
        browser("browser-vm", "Work Profile", None),
    ] {
        let outcome = app.apply_block_request(&request(&app, operation.clone()), &ctx);
        assert_eq!(code(&outcome), block_machines::INVALID_REQUEST, "{operation:?}");
        assert!(
            !message(&outcome).trim().is_empty(),
            "the refusal has words: {outcome:?}"
        );
        assert!(!app.settings_request.is_waiting(), "no card waits after a refusal");
    }
    assert_eq!(
        app.workspace_cards(app.board.panels[0].workspace_id)
            .iter()
            .filter(|panel| panel.block().is_some_and(|card| card.owner_actor.is_none()))
            .count(),
        0,
        "a request that Horizon refuses draws no card"
    );
}

#[test]
fn a_request_of_another_workspace_or_another_host_is_refused_as_the_other_operations_are() {
    let (_temp, ctx, mut app) = app();
    let mut foreign = request(&app, link("server-vm"));
    foreign.host_instance = "another-horizon".into();
    let outcome = app.apply_block_request(&foreign, &ctx);
    assert_eq!(code(&outcome), "wrong_host");

    let mut stranger = request(&app, link("server-vm"));
    stranger.actor = "horizon:not-a-panel".into();
    let outcome = app.apply_block_request(&stranger, &ctx);
    assert_eq!(code(&outcome), "caller_unavailable");
    assert!(!app.settings_request.is_waiting());
}

#[test]
fn the_card_goes_away_with_timed_out_when_its_deadline_passes() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let mut request = request(&app, link("server-vm"));
    // One millisecond of wait after the window that writes the answer.
    request.deadline_at_millis =
        manifest::now_millis() + i64::try_from(HOST_APPLY_TIMEOUT.as_millis()).unwrap_or(0) + 1;
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    let card = asking_card(&mut app);
    let panel = app.board.panel_id_by_local_id(&card);
    std::thread::sleep(std::time::Duration::from_millis(25));

    assert!(app.settle_setting_requests(&ctx), "the deadline answers the request");
    app.settle_browser_host_io();

    assert!(
        panel.is_some_and(|panel| app.board.panel(panel).is_none()),
        "the card is gone"
    );
    assert_eq!(
        answer_at(&root, &request),
        serde_json::json!({"status": "server_link", "answer": "timed_out"})
    );
    assert!(!app.settings_request.is_waiting());
    let _ = id;
    let after = stored(&mut app);
    assert!(!after.server.names_server(), "a wait that ended changes nothing");
}

#[test]
fn a_card_that_the_person_closed_answers_timed_out() {
    let (temp, ctx, mut app) = app();
    let root = temp.path().join("block-panel-requests");
    std::fs::create_dir_all(&root).expect("the queue directory");
    let request = request(&app, browser("browser-vm", "work", None));
    assert!(app.apply_block_request_in(&request, &root, &ctx).is_none());
    let card = asking_card(&mut app);
    let panel = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    let _ = app.close_panel_returning_teardown(panel);

    assert!(app.settle_setting_requests(&ctx), "a card that left the board answers");
    app.settle_browser_host_io();

    assert_eq!(
        answer_at(&root, &request),
        serde_json::json!({"status": "browser_host", "answer": "timed_out"}),
        "a person who closed the card pressed nothing"
    );
    assert_eq!(stored(&mut app).browser_host, BrowserHostConfig::default());
}

#[test]
fn a_press_on_any_other_card_of_horizon_reaches_no_setting() {
    let (_temp, ctx, mut app) = app();
    assert!(
        app.apply_block_request_waiting(&request(&app, link("server-vm")), &ctx)
            .is_none()
    );
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    for button in ["look_again", "open_presets", "close"] {
        app.press_host_card_button(&ctx, id, button);
    }
    assert!(app.settings_request.is_waiting(), "the request still waits");
    assert!(!stored(&mut app).server.names_server(), "and nothing was written");
}

#[test]
fn the_card_does_not_take_the_keyboard_from_the_agent() {
    let (_temp, ctx, mut app) = app();
    let agent = app.board.panels[0].id;
    app.board.focus(agent);
    assert!(
        app.apply_block_request_waiting(&request(&app, link("server-vm")), &ctx)
            .is_none()
    );
    assert_eq!(
        app.board.focused,
        Some(agent),
        "the question does not take the keyboard from the panel the person uses"
    );
}

#[test]
fn the_cards_of_both_settings_are_valid_and_name_their_change() {
    for (operation, title, words) in [
        (
            link("server-vm"),
            SERVER_TITLE,
            [
                "Link this computer to the Horizon server on server-vm?",
                SERVER_SENT,
                SERVER_KEPT,
                "Link",
                "Not now",
            ],
        ),
        (
            browser("browser-vm", "work", None),
            BROWSER_TITLE,
            [
                "Use the browser on browser-vm for new browser panels?",
                BROWSER_USES,
                BROWSER_APPLIES,
                "Use it",
                "Not now",
            ],
        ),
    ] {
        let document = document_of(&operation).expect("the card of this request");
        assert_eq!(validate_document(&document), Ok(()), "{title} is a valid card");
        let text = format!("{document:?}");
        for word in words {
            assert!(text.contains(word), "{title} says {word}: {text}");
        }
        assert!(
            !document
                .components
                .iter()
                .any(|component| matches!(component, horizon_browser_control::block::Component::Link { .. })),
            "a setting card opens no page"
        );
    }
}

/// The browser of the machine that a card confirmed serves every browser panel
/// that a create makes after the press, on both create paths, and an open panel
/// keeps the browser that it has.
#[test]
fn a_pressed_use_it_serves_every_browser_panel_that_a_create_makes_after_it() {
    let (_temp, ctx, mut app) = app();
    let workspace = app.board.panels[0].workspace_id;
    // A panel that is open before the press keeps the browser of this computer.
    let open = app
        .create_panel_with_options_beside(browser_panel_options(&app, true), workspace, None)
        .expect("a local browser panel");
    assert!(
        !browser_of(&app, open).is_attached(),
        "the panel that the person opened before the press runs here"
    );

    let request = request(&app, browser("browser-vm", "work", None));
    assert!(app.apply_block_request_waiting(&request, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    app.settle_browser_host_io();

    let person = app
        .create_panel_with_options_beside(browser_panel_options(&app, true), workspace, None)
        .expect("a browser panel of the person");
    assert_attached(&app, person);

    let agent = app
        .create_agent_child_panel(browser_panel_options(&app, true), workspace, person)
        .expect("a browser panel of an agent");
    assert_attached(&app, agent);

    let still_local = browser_of(&app, open);
    assert!(
        !still_local.is_attached(),
        "the press moves no panel that was open before it"
    );
    assert_eq!(still_local.attached_host_target(), None);
}

/// The engine and the route of the confirmed entry decide the panel: a Firefox
/// host of an SSH target and a Chromium host of a machine of the list both serve
/// the panels that a create makes after the press.
#[test]
fn a_pressed_use_it_serves_a_firefox_host_and_a_machine_of_the_list() {
    let (_temp, ctx, mut app) = app();
    let workspace = app.board.panels[0].workspace_id;
    plant_machine(&mut app, "rented-1");
    let on_machine_card = request(&app, browser_on_machine("rented-1", "work"));
    assert!(app.apply_block_request_waiting(&on_machine_card, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    app.settle_browser_host_io();

    let rented = app
        .create_panel_with_options_beside(browser_panel_options(&app, true), workspace, None)
        .expect("a browser panel of a machine of the list");
    let of_machine = browser_of(&app, rented);
    assert_eq!(
        of_machine.attached_host_target(),
        Some("peters@10.0.0.5"),
        "the panel reaches the machine at its address and its user"
    );
    assert_eq!(of_machine.attached_host_profile(), Some("work"));
    assert!(
        of_machine.attached_host_login().is_some(),
        "the connect carries the machine key of this computer"
    );

    let firefox_card = request(&app, browser("browser-vm", "mail", Some(BrowserEngine::Firefox)));
    assert!(app.apply_block_request_waiting(&firefox_card, &ctx).is_none());
    let card = asking_card(&mut app);
    let id = app.board.panel_id_by_local_id(&card).expect("the card of the request");
    app.press_host_card_button(&ctx, id, APPLY);
    app.settle_browser_host_io();

    let firefox = app
        .create_agent_child_panel(browser_panel_options(&app, true), workspace, rented)
        .expect("a browser panel of a Firefox host");
    let driver = browser_of(&app, firefox);
    assert!(
        driver.is_host_driver(),
        "the page of a Firefox host runs behind its driver"
    );
    assert_eq!(driver.host_driver_target(), Some("browser-vm"));
    assert_eq!(driver.host_driver_profile(), Some("mail"));
}

/// The browser of a panel of this test board.
fn browser_of(app: &HorizonApp, panel: horizon_core::PanelId) -> &horizon_core::browser::BrowserPanelState {
    app.board
        .panel(panel)
        .and_then(|panel| panel.browser())
        .expect("a browser panel")
}

/// The panel runs on the Chromium of the host that the press confirmed.
fn assert_attached(app: &HorizonApp, panel: horizon_core::PanelId) {
    let browser = browser_of(app, panel);
    assert!(browser.is_attached(), "the panel runs on the browser of its host");
    assert_eq!(browser.attached_host_target(), Some("browser-vm"));
    assert_eq!(browser.attached_host_profile(), Some("work"));
}

/// Writes the record of `name` and the key that this computer binds to it into the
/// machine list of `app`.
fn plant_machine(app: &mut HorizonApp, name: &str) {
    use horizon_core::machines::{MachineRegistry, Machines};
    use horizon_core::{Machine, MachineKeys};
    let registry = MachineRegistry::from_home(app.session_store.home());
    let list = Machines::load(registry.clone()).expect("the machine list of this computer");
    let (list, _) = list
        .with_record(
            Machine {
                name: name.to_owned(),
                host: "10.0.0.5".to_owned(),
                user: Some("peters".to_owned()),
                ..Machine::default()
            },
            Some(name),
        )
        .expect("a new record");
    list.save().expect("the record is written");
    app.machine_list = Machines::load(registry).expect("the machine list of this computer");
    let directory = MachineKeys::from_home(app.session_store.home()).key_directory();
    std::fs::create_dir_all(&directory).expect("the key directory");
    std::fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the key");
}
