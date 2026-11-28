//! The three operations of the front desk: the work items that it reads, the
//! proposal that it offers, and the message that it passes to one work item,
//! judged on a test board with a stand-in program.
//!
//! The programme of every front desk of these tests is a `/bin/sh` script that
//! the test writes into a folder of its own: it records each line that it reads in
//! one file, answers the lines that name `hello` or `quiet`, and fails the turn of
//! a line that names `fail`. No test starts an agent program.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::*;
use crate::app::orchestrator_dock::FRONT_DESK;
use crate::app::orchestrator_dock::proposal::Choice;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_browser_control::manifest::block::{MAX_PROPOSAL_ITEMS, ProposedItem};
use horizon_core::agent_bridge::{Entry, ProposalState};
use horizon_core::{RuntimeState, StartupDecision};

type TestApp = (tempfile::TempDir, Context, HorizonApp);

/// The local ids of the two workspaces of the board.
const FIRST: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";
const SECOND: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f3a4b5c6d";

/// The session that every stand-in of these tests names.
const SESSION: &str = "00000000-0000-4000-8000-000000000001";

/// The stand-in of a front desk: it records every line that it reads in `@LINES@`,
/// answers each line with one text and the result of the turn, fails the turn of a
/// line that names `fail`, and holds the turn of a line that names `quiet` for a
/// moment, so a test owns a conversation that is busy.
const STAND_IN: &str = r#"while IFS= read -r line; do
  printf '%s\n' "$line" >> '@LINES@'
  case "$line" in
    *quiet*) sleep 0.4 ;;
  esac
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
  printf '%s\n' '{"type":"assistant","message":{"id":"m-1","content":[{"type":"text","text":"@ANSWER@"}]}}'
  if [ -n "$(printf '%s' "$line" | grep fail)" ]; then
    printf '%s\n' '{"type":"result","is_error":true,"duration_ms":5}'
  else
    printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
  fi
done
"#;

/// The board of these tests: two workspaces, each with one agent panel.
fn app() -> TestApp {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state(FIRST, [0.0, 0.0]),
            editor_workspace_state(SECOND, [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    crate::app::block_machines::settle_machine_load(&mut app);
    (temp, ctx, app)
}

/// A request of the actor of the front desk.
fn front_desk_request(operation: Operation) -> Request {
    Request {
        request_id: "test-front-desk".into(),
        actor: actor::FRONT_DESK.into(),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// The code of one refusal.
fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// The stand-in program of the dock, which records its lines in `lines`.
fn stand_in(folder: &Path, lines: &Path) -> PathBuf {
    stand_in_answering(folder, lines, "the front desk answers")
}

/// The same stand-in, with `answer` as the text of each turn.
fn stand_in_answering(folder: &Path, lines: &Path, answer: &str) -> PathBuf {
    let body = STAND_IN
        .replace("@LINES@", &lines.display().to_string())
        .replace("@SESSION@", SESSION)
        .replace("@ANSWER@", answer);
    horizon_core::stand_in_program_in(folder, "front-desk-stand-in.sh", &body).expect("the stand-in program")
}

/// Runs frames until `wanted` holds, and names what the test waited for.
fn wait_for(app: &mut HorizonApp, what: &str, wanted: impl Fn(&HorizonApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !wanted(app) {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        app.poll_orchestrator_dock(&Context::default());
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Starts the program of the front desk with a message, and waits until its bridge
/// runs. It answers the folder that holds the record of its lines.
fn running_front_desk(temp: &tempfile::TempDir, ctx: &Context, app: &mut HorizonApp) -> PathBuf {
    let lines = temp.path().join("lines.txt");
    let program = stand_in(temp.path(), &lines);
    app.orchestrator_dock.set_program(program);
    assert!(app.say_to_orchestrator(ctx, FRONT_DESK, "hello"));
    wait_for(app, "the front desk runs", |app| {
        app.orchestrator_dock.running(FRONT_DESK) && !app.orchestrator_dock.busy(FRONT_DESK)
    });
    lines
}

/// The lines that the program of the front desk read, in order.
fn lines_of(lines: &Path) -> Vec<String> {
    fs::read_to_string(lines)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The number of the lines of `lines` that hold `text`.
fn count_of(lines: &Path, text: &str) -> usize {
    lines_of(lines).iter().filter(|line| line.contains(text)).count()
}

/// The items of the proposal of these tests.
fn two_items() -> Vec<ProposedItem> {
    vec![
        ProposedItem {
            title: "Fix login redirect".to_owned(),
            task: "Fix issue 1987. Done means the test passes.".to_owned(),
            folder: None,
        },
        ProposedItem {
            title: "Faster cold start".to_owned(),
            task: "question: make the cold start faster.".to_owned(),
            folder: None,
        },
    ]
}

/// One proposal, answered as the tool answers it.
fn propose(app: &mut HorizonApp, ctx: &Context, items: Vec<ProposedItem>) -> String {
    match app.apply_block_request(&front_desk_request(Operation::WorkItemPropose { items }), ctx) {
        Outcome::Proposed { proposal, text } => {
            assert_eq!(text, "The person decides. You get one message when they do.");
            proposal
        }
        other => panic!("expected one proposal: {other:?}"),
    }
}

/// The proposals of the conversation of the front desk, in order.
fn proposals(app: &HorizonApp) -> Vec<(String, ProposalState)> {
    app.orchestrator_dock
        .conversation_entries(FRONT_DESK)
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Proposal { id, state, .. } => Some((id, state)),
            _ => None,
        })
        .collect()
}

/// The reference rows of the work items of the conversation of the front desk.
fn work_item_rows(app: &HorizonApp) -> Vec<(String, String)> {
    app.orchestrator_dock
        .conversation_entries(FRONT_DESK)
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::WorkItem { work_item, name } => Some((work_item, name)),
            _ => None,
        })
        .collect()
}

/// The notices of the conversation of the front desk, in order.
fn notices(app: &HorizonApp) -> Vec<String> {
    app.orchestrator_dock
        .conversation_entries(FRONT_DESK)
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Notice { text } => Some(text),
            _ => None,
        })
        .collect()
}

/// Waits until the board holds the work item `name`, and answers its local id.
fn wait_work_item(app: &mut HorizonApp, name: &str) -> String {
    wait_for(app, name, |app| workspace_named(app, name).is_some());
    workspace_named(app, name)
        .and_then(|workspace| app.board.workspace(workspace))
        .map(|workspace| workspace.local_id.clone())
        .expect("the work item of the test")
}

/// The workspace of the board that carries `name`.
fn workspace_named(app: &HorizonApp, name: &str) -> Option<horizon_core::WorkspaceId> {
    app.board
        .workspaces
        .iter()
        .find(|workspace| workspace.name == name)
        .map(|workspace| workspace.id)
}

#[test]
fn only_the_running_program_of_the_front_desk_reaches_the_work_item_operations() {
    let (temp, ctx, mut app) = app();
    // No bridge of the front desk runs yet, so its actor is no caller.
    let refusal = app.apply_block_request(&front_desk_request(Operation::WorkItems), &ctx);
    assert_eq!(code(&refusal), "caller_unavailable", "{refusal:?}");

    running_front_desk(&temp, &ctx, &mut app);
    let accepted = app.apply_block_request(&front_desk_request(Operation::WorkItems), &ctx);
    assert!(
        matches!(accepted, Outcome::WorkItems { .. }),
        "the running program of the front desk reads the work items: {accepted:?}"
    );

    // An orchestrator actor and a panel actor reach no work item operation.
    let local_id = app.board.workspaces[0].local_id.clone();
    let mut request = front_desk_request(Operation::WorkItems);
    request.actor = actor::of_workspace(&local_id).expect("the actor of a workspace");
    assert_eq!(code(&app.apply_block_request(&request, &ctx)), "caller_unavailable");
    let panel = app.board.panels[0].local_id.clone();
    request.actor = format!("horizon:{panel}");
    assert_eq!(code(&app.apply_block_request(&request, &ctx)), "caller_unavailable");
    let orchestrator = Operation::WorkItemPropose { items: two_items() };
    request.operation = orchestrator;
    assert_eq!(code(&app.apply_block_request(&request, &ctx)), "caller_unavailable");
}

#[test]
fn the_front_desk_reaches_no_other_operation_of_the_tool() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    for operation in [
        Operation::List,
        Operation::Presets,
        Operation::Machines,
        Operation::SecretNames,
        Operation::ServerStatus,
        Operation::BrowserHostStatus,
        Operation::Create {
            title: Some("Plan".into()),
            document: serde_json::json!({"components": [{"type": "heading", "text": "Plan"}]}),
        },
        Operation::OpenPanel {
            preset: "shell".into(),
            directory: "/tmp".into(),
            title: None,
            prompt: None,
        },
        Operation::MachineKey { name: "cloud-1".into() },
        Operation::ServerLinkRequest {
            ssh_target: Some("server".into()),
            machine: None,
            command: None,
        },
    ] {
        let refusal = app.apply_block_request(&front_desk_request(operation.clone()), &ctx);
        assert_eq!(
            code(&refusal),
            "caller_unavailable",
            "the front desk is refused {operation:?}: {refusal:?}"
        );
    }
    assert_eq!(
        app.board.panels.len(),
        2,
        "a refused create draws no card in the workspace of the front desk"
    );
}

#[test]
fn a_proposal_outside_its_bounds_is_refused_whole_and_a_second_replaces_the_first() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    let mut seven = two_items();
    while seven.len() <= MAX_PROPOSAL_ITEMS {
        seven.push(two_items()[0].clone());
    }
    for items in [
        Vec::new(),
        seven,
        vec![ProposedItem {
            title: "  ".into(),
            task: "do it".into(),
            folder: None,
        }],
        vec![ProposedItem {
            title: "t".repeat(61),
            task: "do it".into(),
            folder: None,
        }],
        vec![ProposedItem {
            title: "a".into(),
            task: "t".repeat(4_001),
            folder: None,
        }],
        vec![ProposedItem {
            title: "a".into(),
            task: "do it".into(),
            folder: Some("work".into()),
        }],
    ] {
        let refusal = app.apply_block_request(&front_desk_request(Operation::WorkItemPropose { items }), &ctx);
        assert_eq!(code(&refusal), "invalid_request", "{refusal:?}");
        assert!(proposals(&app).is_empty(), "a refused proposal adds no entry");
    }

    let first = propose(&mut app, &ctx, two_items());
    let workspaces = app.board.workspaces.len();
    assert!(
        workspace_named(&app, "Fix login redirect").is_none(),
        "a proposal starts nothing"
    );
    assert_eq!(app.board.workspaces.len(), workspaces);
    let second = propose(&mut app, &ctx, two_items());
    assert_eq!(
        proposals(&app),
        vec![(first, ProposalState::Replaced), (second, ProposalState::Open)],
        "a newer proposal replaces the one that is still open"
    );
}

#[test]
fn a_press_on_start_makes_one_workspace_for_each_item_and_sends_its_task() {
    let (temp, ctx, mut app) = app();
    let lines = running_front_desk(&temp, &ctx, &mut app);
    let active = app.board.active_workspace.expect("an active workspace");
    let shown = app.orchestrator_dock.front_desk().key(&app.board).to_owned();
    // A folder that is a directory of this computer, and one that is not.
    let folder = temp.path().join("work");
    fs::create_dir_all(&folder).expect("the folder of the test");
    let missing = temp.path().join("not-there");
    let mut items = two_items();
    items[0].folder = Some(folder.display().to_string());
    items[1].folder = Some(missing.display().to_string());
    let id = propose(&mut app, &ctx, items);

    app.press_proposal(&ctx, &id, Choice::Start);
    wait_for(&mut app, "the proposal starts", |app| {
        workspace_named(app, "Faster cold start").is_some()
    });
    wait_for(&mut app, "both work items start", |app| {
        workspace_named(app, "Fix login redirect").is_some()
    });

    let first = workspace_named(&app, "Fix login redirect").expect("the first work item");
    let second = workspace_named(&app, "Faster cold start").expect("the second work item");
    assert_eq!(
        app.board.workspace(first).and_then(|workspace| workspace.cwd.clone()),
        Some(folder.clone()),
        "the folder of an item that the check found is the folder of its workspace"
    );
    assert_eq!(
        app.board.workspace(second).and_then(|workspace| workspace.cwd.clone()),
        None,
        "a folder that the check did not find leaves the workspace without one"
    );
    assert!(
        app.board
            .workspace(first)
            .is_some_and(|workspace| workspace.panels.is_empty()),
        "a work item starts empty, and the board keeps it while it is"
    );

    // The task of each item is the first message of the orchestrator of its
    // workspace, and the front desk passed it.
    let first_local = app
        .board
        .workspaces
        .iter()
        .find(|w| w.id == first)
        .expect("the workspace")
        .local_id
        .clone();
    let second_local = app
        .board
        .workspaces
        .iter()
        .find(|w| w.id == second)
        .expect("the workspace")
        .local_id
        .clone();
    let sent = app.orchestrator_dock.person_messages(&first_local);
    assert_eq!(sent.len(), 1, "one message of the person: {sent:?}");
    assert!(sent[0].starts_with("Fix issue 1987."), "{sent:?}");
    assert_eq!(app.orchestrator_dock.person_messages(&second_local).len(), 1);

    // The conversation of the front desk names the press, one row for each work
    // item, and the folder that it did not find.
    let notices = notices(&app);
    assert!(
        notices.iter().any(|text| text == "You started 2 work items"),
        "{notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|text| text == "Faster cold start: the folder was not found, so it starts with none"),
        "{notices:?}"
    );
    assert_eq!(
        work_item_rows(&app),
        vec![
            (first_local.clone(), "Fix login redirect".to_owned()),
            (second_local, "Faster cold start".to_owned())
        ]
    );
    assert_eq!(proposals(&app), vec![(id.clone(), ProposalState::Started)]);

    // The active workspace and the conversation that the dock shows do not change.
    assert_eq!(app.board.active_workspace, Some(active));
    assert_eq!(app.orchestrator_dock.front_desk().key(&app.board), shown);

    // The program of the front desk reads the press once.
    wait_for(&mut app, "the line of the press", |app| {
        !lines_of(&lines).is_empty() && !app.orchestrator_dock.busy(FRONT_DESK)
    });
    assert_eq!(
        count_of(&lines, "The person started the work items of proposal"),
        1,
        "one line for one press: {:?}",
        lines_of(&lines)
    );
}

#[test]
fn a_press_on_not_now_leaves_the_board_as_it_is_and_names_the_press() {
    let (temp, ctx, mut app) = app();
    let lines = running_front_desk(&temp, &ctx, &mut app);
    let workspaces = app.board.workspaces.len();
    let id = propose(&mut app, &ctx, two_items());

    app.press_proposal(&ctx, &id, Choice::Decline);
    wait_for(&mut app, "the line of the press", |app| {
        !app.orchestrator_dock.busy(FRONT_DESK)
    });

    assert_eq!(app.board.workspaces.len(), workspaces, "a decline starts no workspace");
    assert_eq!(proposals(&app), vec![(id, ProposalState::Declined)]);
    assert!(
        notices(&app).iter().any(|text| text == "Not started"),
        "{:?}",
        notices(&app)
    );
    assert!(work_item_rows(&app).is_empty());
    assert_eq!(
        count_of(&lines, "The person did not start the work items of proposal"),
        1
    );
}

#[test]
fn the_line_of_a_press_waits_for_the_end_of_a_busy_turn() {
    let (temp, ctx, mut app) = app();
    let lines = running_front_desk(&temp, &ctx, &mut app);
    // A line that the program answers late makes the conversation busy for a
    // moment, so the press falls inside a turn of the front desk.
    assert!(app.say_to_orchestrator(&ctx, FRONT_DESK, "quiet"));
    let id = propose(&mut app, &ctx, two_items());
    app.press_proposal(&ctx, &id, Choice::Start);
    assert!(
        app.orchestrator_dock.busy(FRONT_DESK),
        "the turn of the person still runs"
    );
    assert_eq!(
        count_of(&lines, "The person started the work items of proposal"),
        0,
        "the line of the press waits for the end of that turn"
    );

    wait_for(&mut app, "the line after the turn", |_app| {
        count_of(&lines, "The person started the work items of proposal") == 1
    });
    // One more pass sends no second line.
    for _ in 0..20 {
        app.poll_orchestrator_dock(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        count_of(&lines, "The person started the work items of proposal"),
        1,
        "one line once: {:?}",
        lines_of(&lines)
    );
}

#[test]
fn a_press_on_start_reads_no_folder_on_the_frame_that_presses() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    let folder = temp.path().join("work");
    fs::create_dir_all(&folder).expect("the folder of the test");
    let mut items = two_items();
    items[0].folder = Some(folder.display().to_string());
    let id = propose(&mut app, &ctx, items);

    // No worker of the dock takes a job, so the check of the folders runs
    // nowhere: the frame that presses returns at once, starts no work item, and
    // writes the notice of a press that started nothing.
    app.orchestrator_dock.hold_worker();
    app.press_proposal(&ctx, &id, Choice::Start);
    assert!(workspace_named(&app, "Fix login redirect").is_none());
    assert_eq!(proposals(&app), vec![(id, ProposalState::Open)]);
    assert!(
        notices(&app).iter().any(|text| text == "The work items did not start."),
        "{:?}",
        notices(&app)
    );
    app.orchestrator_dock.take_jobs_again();
}

#[test]
fn the_read_of_the_work_items_names_the_state_the_first_question_and_the_last_text() {
    let (temp, ctx, mut app) = app();
    // The front desk runs, and the workspace of the first local id runs its
    // orchestrator too, whose card asks the person a question. The stand-in answers
    // a text above the bound of the last text, so the cut of the answer shows.
    let long = format!("a {}", "long answer ".repeat(40));
    let lines = temp.path().join("lines.txt");
    let program = stand_in_answering(temp.path(), &lines, &long);
    app.orchestrator_dock.set_program(program);
    assert!(app.say_to_orchestrator(&ctx, FRONT_DESK, "hello"));
    wait_for(&mut app, "the front desk runs", |app| {
        app.orchestrator_dock.running(FRONT_DESK) && !app.orchestrator_dock.busy(FRONT_DESK)
    });
    let first_local = app.board.workspaces[0].local_id.clone();
    assert!(app.say_to_orchestrator(&ctx, &first_local, "hello"));
    wait_for(&mut app, "the orchestrator of the work item runs", |app| {
        app.orchestrator_dock.running(&first_local)
    });
    wait_for(&mut app, "the turn of the work item", |app| {
        !app.orchestrator_dock.busy(&first_local)
    });
    let first = app.board.workspaces[0].id;
    crate::app::sidebar::inbox::asking_card(&mut app, first, &"q".repeat(200), &[("go", "Go")]);
    let second_local = app.board.workspaces[1].local_id.clone();

    let Outcome::WorkItems { items } = app.apply_block_request(&front_desk_request(Operation::WorkItems), &ctx) else {
        panic!("the read of the work items");
    };
    assert_eq!(items.len(), 2, "one entry for each workspace of the board");
    assert_eq!(items[0].work_item, first_local);
    assert_eq!(items[0].name, FIRST, "the name of the workspace of the work item");
    assert_eq!(
        items[0].state, "needs_you",
        "a card of the work item asks the person a question"
    );
    let question = items[0].question.as_deref().expect("the first question of the card");
    assert_eq!(question.chars().count(), 160, "the question is cut to its bound");
    let last = items[0].last.as_deref().expect("the last text of the orchestrator");
    assert_eq!(
        last.chars().count(),
        200,
        "the last text of a work item is cut to its bound"
    );
    assert_eq!(items[1].work_item, second_local);
    assert_eq!(items[1].state, "idle");
    assert_eq!(items[1].question, None);
    assert_eq!(
        items[1].last, None,
        "a work item whose orchestrator said nothing answers no last text"
    );
    assert_eq!(items[1].machine, None, "a workspace of this computer names no machine");
}

#[test]
fn the_read_of_the_work_items_answers_at_most_fifty_workspaces() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    while app.board.workspaces.len() < 52 {
        let _ = app.board.create_workspace("more");
    }
    let Outcome::WorkItems { items } = app.apply_block_request(&front_desk_request(Operation::WorkItems), &ctx) else {
        panic!("the read of the work items");
    };
    assert_eq!(items.len(), 50, "the read stops at the fiftieth work item");
    assert_eq!(
        items[0].work_item, app.board.workspaces[0].local_id,
        "the read keeps the order of the sidebar"
    );
}

#[test]
fn a_message_of_the_front_desk_passes_to_one_work_item_with_its_caption() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    let work_item = app.board.workspaces[0].local_id.clone();

    // An id that no workspace holds, a conversation that runs a turn, and a second
    // message inside the interval: none of them sends anything.
    let unknown = app.apply_block_request(
        &front_desk_request(Operation::WorkItemMessage {
            work_item: "11111111-2222-4333-8444-555555555555".into(),
            text: "hello".into(),
        }),
        &ctx,
    );
    assert_eq!(code(&unknown), "not_found");
    app.orchestrator_dock.begin_turn_for_test(&work_item);
    let busy = app.apply_block_request(
        &front_desk_request(Operation::WorkItemMessage {
            work_item: work_item.clone(),
            text: "hello".into(),
        }),
        &ctx,
    );
    assert_eq!(code(&busy), "busy");
    assert_eq!(app.orchestrator_dock.person_messages(&work_item).len(), 1);

    // The first message of an idle work item goes out, marked with the caption of
    // the front desk.
    app.orchestrator_dock.end_turn_for_test(&work_item);
    let sent = app.apply_block_request(
        &front_desk_request(Operation::WorkItemMessage {
            work_item: work_item.clone(),
            text: "start with the redirect".into(),
        }),
        &ctx,
    );
    assert!(
        matches!(sent, Outcome::WorkItemMessage { .. }),
        "the message of the front desk passes: {sent:?}"
    );
    let entries = app.orchestrator_dock.conversation_entries(&work_item);
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            Entry::Person { text, via_front_desk: true } if text == "start with the redirect"
        )),
        "the message carries the mark of the front desk: {entries:?}"
    );

    // The turn of that message ends, and the next message inside the interval
    // still waits.
    app.orchestrator_dock.end_turn_for_test(&work_item);
    let soon = app.apply_block_request(
        &front_desk_request(Operation::WorkItemMessage {
            work_item: work_item.clone(),
            text: "and the test".into(),
        }),
        &ctx,
    );
    assert_eq!(code(&soon), "too_soon", "a second message inside ten seconds waits");
}

#[test]
fn a_work_item_that_the_front_desk_started_names_its_change_of_state_once() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    let id = propose(&mut app, &ctx, two_items());
    app.press_proposal(&ctx, &id, Choice::Start);
    let work_item = wait_work_item(&mut app, "Faster cold start");
    wait_for(&mut app, "the work items of the proposal", |app| {
        workspace_named(app, "Fix login redirect").is_some()
    });

    // A turn of the work item runs, and the person reads the front desk: the row of
    // the work item says so, and a turn that runs names no notice.
    app.orchestrator_dock.begin_turn_for_test(&work_item);
    app.sidebar_list.count_inbox_change();
    app.refresh_sidebar_rows(Instant::now());
    app.poll_orchestrator_dock(&ctx);
    assert_eq!(
        app.orchestrator_dock.work_item_word(&work_item),
        Some("working"),
        "the turn of the work item runs"
    );
    assert!(
        !notices(&app)
            .iter()
            .any(|text| text.ends_with("needs you") || text.ends_with("finished") || text.ends_with("failed")),
        "a turn that runs names no notice: {:?}",
        notices(&app)
    );

    // The turn ends while the person looks at the front desk, so the work item says
    // that it finished, and one notice names it.
    app.orchestrator_dock.end_turn_for_test(&work_item);
    app.sidebar_list.count_inbox_change();
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(2));
    app.poll_orchestrator_dock(&ctx);
    assert_eq!(app.orchestrator_dock.work_item_word(&work_item), Some("done"));
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|text| *text == "Faster cold start finished")
            .count(),
        1,
        "one notice for one change: {:?}",
        notices(&app)
    );
    // A second pass of the same state adds no notice.
    app.poll_orchestrator_dock(&ctx);
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|text| *text == "Faster cold start finished")
            .count(),
        1
    );

    // A card of that work item asks the person a question, and the front desk reads
    // it as one more notice.
    let workspace = workspace_named(&app, "Faster cold start").expect("the work item");
    crate::app::sidebar::inbox::asking_card(&mut app, workspace, "Which route?", &[("go", "Go")]);
    app.sidebar_list.count_inbox_change();
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(3));
    app.poll_orchestrator_dock(&ctx);
    assert_eq!(
        app.orchestrator_dock.work_item_word(&work_item),
        Some("needs you"),
        "the row of a work item whose orchestrator asks a question says the words of a person"
    );
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|text| *text == "Faster cold start needs you")
            .count(),
        1,
        "{:?}",
        notices(&app)
    );
}

/// A real run showed it: the person looked at the conversation of a work item
/// when its turn ended, so the row carried no `done`, and the front desk never
/// heard that the work item finished.
#[test]
fn a_turn_that_ends_on_the_screen_is_still_one_finished_notice_of_the_front_desk() {
    let (temp, ctx, mut app) = app();
    running_front_desk(&temp, &ctx, &mut app);
    let id = propose(&mut app, &ctx, two_items());
    app.press_proposal(&ctx, &id, Choice::Start);
    let work_item = wait_work_item(&mut app, "Faster cold start");
    let workspace = workspace_named(&app, "Faster cold start").expect("the work item");

    // The person opens the conversation of the work item, and its turn runs.
    app.board.focus_workspace(workspace);
    app.orchestrator_dock.front_desk_mut().show_workspace();
    app.orchestrator_dock.begin_turn_for_test(&work_item);
    app.sidebar_list.count_inbox_change();
    app.refresh_sidebar_rows(Instant::now());
    app.poll_orchestrator_dock(&ctx);

    // The turn ends on the screen: the row says nothing, because the person saw it.
    app.orchestrator_dock.end_turn_for_test(&work_item);
    app.sidebar_list.count_inbox_change();
    app.refresh_sidebar_rows(Instant::now() + Duration::from_secs(2));
    app.poll_orchestrator_dock(&ctx);
    assert_eq!(
        app.orchestrator_dock.work_item_word(&work_item),
        None,
        "a turn that the person saw end leaves no word on the row"
    );
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|text| *text == "Faster cold start finished")
            .count(),
        1,
        "the front desk hears the end of a turn that it did not see: {:?}",
        notices(&app)
    );
    // A second pass of the same state adds no notice.
    app.poll_orchestrator_dock(&ctx);
    assert_eq!(
        notices(&app)
            .iter()
            .filter(|text| *text == "Faster cold start finished")
            .count(),
        1
    );
}
