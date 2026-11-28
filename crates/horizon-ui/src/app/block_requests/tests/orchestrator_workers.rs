//! The four worker operations of an orchestrator: the caller that reaches each of
//! them, the bounds of the three texts, the start of one worker of the chip, the
//! fifth worker, the request for a program outside the chip, the read of the
//! workers, the message to one of them, and the report of a worker.
//!
//! The program of a conversation of these tests is a `/bin/sh` script that the test
//! writes into a folder of its own: it names its session, records each line that it
//! reads in one file and holds `quiet` for a moment, so a test owns a bridge that
//! works. No test starts an agent program.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::app::block_requests::workers::WORKER_MARK;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_browser_control::manifest::block_workers::{
    MAX_WAITING_REPORTS, MAX_WORKER_TEXT_CHARS, MAX_WORKERS, REPORT_SENTENCE, WORKER_INTERVAL_MILLIS, WorkerSummary,
};
use horizon_core::agent_bridge::{Entry, WorkerAskState, actor};
use horizon_core::{PresetConfig, RuntimeState, StartupDecision};

type TestApp = (tempfile::TempDir, Context, HorizonApp);

/// The local ids of the two workspaces of the board.
const FIRST: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";
const SECOND: &str = "9a8b7c6d-5e4f-4a3b-9c2d-1e0f3a4b5c6d";

/// The session that the stand-in names.
const SESSION: &str = "00000000-0000-4000-8000-000000000001";

/// The stand-in of an orchestrator: it records each line that it reads in
/// `@LINES@`, answers one text and one result, and holds the turn of a line that
/// names `quiet` for a moment.
const ORCHESTRATOR: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '@LINES@'
  case "$line" in
    *quiet*) sleep 0.4 ;;
  esac
  printf '%s\n' '{"type":"assistant","message":{"id":"m-1","content":[{"type":"text","text":"the orchestrator answers"}]}}'
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
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

/// The local id of the workspace at `index` of the board.
fn workspace_local_id(app: &HorizonApp, index: usize) -> String {
    app.board
        .workspaces
        .get(index)
        .map(|workspace| workspace.local_id.clone())
        .expect("the workspace of the test board")
}

/// A request of the orchestrator of the workspace at `index`.
fn orchestrator_request(app: &HorizonApp, index: usize, operation: Operation) -> Request {
    Request {
        request_id: "test-orchestrator-worker".into(),
        actor: actor::of_workspace(&workspace_local_id(app, index)).expect("the actor of a workspace"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// A request of the agent panel with `local_id`.
fn panel_request(local_id: &str, operation: Operation) -> Request {
    Request {
        request_id: "test-worker-panel".into(),
        actor: format!("horizon:{local_id}"),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

/// A request of the front desk.
fn front_desk_request(operation: Operation) -> Request {
    Request {
        request_id: "test-worker-front-desk".into(),
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

/// The words of one refusal.
fn message(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { message, .. } => message,
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// A preset of an agent kind whose program is a stand-in of `folder`.
fn agent_preset(folder: &Path, name: &str) -> PresetConfig {
    let program = horizon_core::stand_in_program_in(
        folder,
        &format!("{name}-stand-in"),
        "#!/bin/sh\nprintf 'worker ready\\n'\nwhile IFS= read -r line; do printf 'worker read: %s\\n' \"$line\"; done",
    )
    .expect("the program of the preset");
    PresetConfig {
        name: name.to_owned(),
        alias: None,
        kind: PanelKind::Claude,
        command: Some(program.display().to_string()),
        args: Vec::new(),
        resume: horizon_core::PanelResume::Fresh,
        ssh_connection: None,
    }
}

/// Adds the preset `Worker Codex` to the settings of the test app, and answers its
/// name. The name is not one of the presets that the default settings hold, so the
/// test reads the preset of its own program.
fn with_preset(app: &mut HorizonApp, folder: &Path) -> String {
    let preset = agent_preset(folder, "Worker Codex");
    let name = preset.name.clone();
    app.template_config.presets.push(preset);
    name
}

/// Says that the dock runs the program of the workspace at `index`, so its actor is
/// a caller of this host without an agent program behind it.
fn running(app: &mut HorizonApp, index: usize) -> String {
    let local_id = workspace_local_id(app, index);
    app.orchestrator_dock.mark_running_for_test(&local_id);
    local_id
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

/// One start of a worker of the workspace at `index`, as the tool answers it.
fn start(app: &mut HorizonApp, ctx: &Context, index: usize, program: &str, task: &str) -> Outcome {
    app.apply_block_request(
        &orchestrator_request(
            app,
            index,
            Operation::WorkerStart {
                program: program.to_owned(),
                task: task.to_owned(),
            },
        ),
        ctx,
    )
}

/// The entries of the conversation of `local_id`.
fn entries_of(app: &HorizonApp, local_id: &str) -> Vec<Entry> {
    app.orchestrator_dock.conversation_entries(local_id)
}

/// The rows of the workers of the conversation of `local_id`.
fn rows_of(app: &HorizonApp, local_id: &str) -> Vec<Entry> {
    entries_of(app, local_id)
        .into_iter()
        .filter(|entry| {
            matches!(
                entry,
                Entry::WorkerStarted { .. } | Entry::WorkerAsk { .. } | Entry::WorkerMessage { .. }
            )
        })
        .collect()
}

#[test]
fn only_the_orchestrator_of_a_workspace_and_its_workers_reach_the_worker_operations() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let local_id = running(&mut app, 0);
    app.orchestrator_dock.allow_worker_program(&local_id, &program);

    // The four operations of an orchestrator that runs its program.
    assert!(
        matches!(
            start(&mut app, &ctx, 0, &program, "fix it"),
            Outcome::WorkerStarted { .. }
        ),
        "the running orchestrator of a workspace starts a worker"
    );
    let read = app.apply_block_request(&orchestrator_request(&app, 0, Operation::Workers), &ctx);
    assert!(matches!(read, Outcome::Workers { .. }), "{read:?}");
    let worker = app.board.panels.last().expect("the worker").local_id.clone();
    let sent = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker: worker.clone(),
                text: "next".into(),
            },
        ),
        &ctx,
    );
    assert!(matches!(sent, Outcome::WorkerMessage { .. }), "{sent:?}");

    // A panel that the person opened, and the front desk, reach none of them.
    for operation in [
        Operation::WorkerStart {
            program: program.clone(),
            task: "fix it".into(),
        },
        Operation::Workers,
        Operation::WorkerMessage {
            worker: worker.clone(),
            text: "next".into(),
        },
        Operation::WorkerReport { text: "done".into() },
    ] {
        let panel = app.board.panels[0].local_id.clone();
        let from_panel = app.apply_block_request(&panel_request(&panel, operation.clone()), &ctx);
        assert_eq!(
            code(&from_panel),
            "caller_unavailable",
            "an agent panel of the person is no caller of {operation:?}"
        );
        let from_front_desk = app.apply_block_request(&front_desk_request(operation.clone()), &ctx);
        assert_eq!(
            code(&from_front_desk),
            "caller_unavailable",
            "the front desk is no caller of {operation:?}"
        );
    }

    // An orchestrator actor whose program ended is no caller either.
    let ended = workspace_local_id(&app, 1);
    let request = orchestrator_request(&app, 1, Operation::Workers);
    assert_eq!(
        code(&app.apply_block_request(&request, &ctx)),
        "caller_unavailable",
        "the workspace {ended} runs no program of the dock"
    );

    // A report belongs to a worker of a running orchestrator.
    let report = app.apply_block_request(
        &panel_request(
            &worker,
            Operation::WorkerReport {
                text: "the test passes".into(),
            },
        ),
        &ctx,
    );
    assert!(
        matches!(report, Outcome::WorkerReported { .. }),
        "the worker of a running orchestrator reports: {report:?}"
    );
    let orchestrator_panel = app.board.panels[0].local_id.clone();
    let stranger = app.apply_block_request(
        &panel_request(&orchestrator_panel, Operation::WorkerReport { text: "done".into() }),
        &ctx,
    );
    assert_eq!(
        code(&stranger),
        "caller_unavailable",
        "a panel that is no worker is refused"
    );
}

#[test]
fn a_task_a_message_and_a_report_outside_their_bound_are_refused_whole() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let local_id = running(&mut app, 0);
    app.orchestrator_dock.allow_worker_program(&local_id, &program);
    let panels = app.board.panels.len();
    let long = "t".repeat(MAX_WORKER_TEXT_CHARS + 1);

    let refusal = start(&mut app, &ctx, 0, &program, &long);
    assert_eq!(code(&refusal), "invalid_request", "{refusal:?}");
    assert!(message(&refusal).contains("4001"), "{}", message(&refusal));
    let empty = start(&mut app, &ctx, 0, &program, "   ");
    assert_eq!(code(&empty), "invalid_request");
    assert_eq!(app.board.panels.len(), panels, "a refused task starts no panel");
    assert!(rows_of(&app, &local_id).is_empty(), "a refused task writes no row");

    assert!(matches!(
        start(&mut app, &ctx, 0, &program, &"t".repeat(MAX_WORKER_TEXT_CHARS)),
        Outcome::WorkerStarted { .. }
    ));
    let worker = app.board.panels.last().expect("the worker").local_id.clone();
    let sent = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker: worker.clone(),
                text: long.clone(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&sent), "invalid_request");
    let reported = app.apply_block_request(&panel_request(&worker, Operation::WorkerReport { text: long }), &ctx);
    assert_eq!(code(&reported), "invalid_request");

    // Every field of the four operations is flat text, so an agent program that
    // builds every argument as a string reaches all of them.
    for operation in [
        serde_json::json!({"operation": "worker_start", "program": program, "task": "fix it"}),
        serde_json::json!({"operation": "workers"}),
        serde_json::json!({"operation": "worker_message", "worker": worker, "text": "next"}),
        serde_json::json!({"operation": "worker_report", "text": "done"}),
    ] {
        let parsed: Operation = serde_json::from_value(operation.clone())
            .unwrap_or_else(|error| panic!("the text form of {operation} parses: {error}"));
        assert!(
            matches!(
                parsed,
                Operation::WorkerStart { .. }
                    | Operation::Workers
                    | Operation::WorkerMessage { .. }
                    | Operation::WorkerReport { .. }
            ),
            "the four operations read their own text"
        );
    }
    // A list where a text belongs is refused by the reader of the operation.
    assert!(
        serde_json::from_value::<Operation>(serde_json::json!({
            "operation": "worker_start", "program": program, "task": ["fix it"]
        }))
        .is_err(),
        "a task is one text and no list"
    );
}

#[test]
fn a_worker_of_the_chip_opens_one_panel_with_the_preset_and_the_sentence() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let preset = app
        .template_config
        .presets
        .iter()
        .find(|preset| preset.name == program)
        .cloned()
        .expect("the preset of the test");
    let local_id = running(&mut app, 0);
    app.orchestrator_dock.allow_worker_program(&local_id, &program);
    let folder = temp.path().join("work");
    fs::create_dir_all(&folder).expect("the folder of the workspace");
    let workspace = app.board.workspaces[0].id;
    app.board.set_workspace_directory(workspace, Some(folder.as_path()));
    let panels = app.board.panels.len();
    let focus = app.board.focused;

    let outcome = start(&mut app, &ctx, 0, &program, "Fix the redirect\nand report");

    let Outcome::WorkerStarted { worker, program: named } = outcome else {
        panic!("the start of a worker of the chip opens one panel: {outcome:?}");
    };
    assert_eq!(named, program, "the answer names the preset of the panel");
    assert_eq!(app.board.panels.len(), panels + 1, "one start opens one panel");
    let id = app
        .board
        .panel_id_by_local_id(&worker)
        .expect("the panel of the worker");
    let panel = app.board.panel(id).expect("the panel of the worker");
    assert_eq!(
        panel.launch_command.as_deref(),
        preset.command.as_deref(),
        "the command of the preset is the command of the worker"
    );
    assert_eq!(panel.kind, PanelKind::Claude, "the kind comes from the preset");
    assert_eq!(
        panel.launch_cwd.as_deref(),
        Some(folder.as_path()),
        "the folder of a worker is the folder of the workspace"
    );
    assert_eq!(
        panel.opened_by(),
        Some(WORKER_MARK),
        "the titlebar of a worker carries the mark of Horizon"
    );
    assert_eq!(app.board.focused, focus, "a worker takes no keyboard from the person");
    assert!(
        app.agent_opens.panels.holds_prompts(),
        "the task of the worker waits for its program"
    );
    assert_eq!(
        rows_of(&app, &local_id),
        vec![Entry::WorkerStarted {
            panel: worker.clone(),
            program: program.clone(),
            task: "Fix the redirect".to_owned(),
        }],
        "the conversation of the orchestrator names the worker and its task"
    );

    // The program of the worker reads one line: the task and the sentence of the
    // report.
    app.settle_panel_starts();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut screen;
    loop {
        app.board.process_output();
        let _ = app.write_ready_prompts();
        screen = app
            .board
            .panel(id)
            .and_then(Panel::terminal)
            .map(horizon_core::Terminal::viewport_text)
            .unwrap_or_default();
        if screen.iter().any(|row| row.contains("worker read:"))
            && screen.iter().any(|row| row.contains("worker_report"))
        {
            break;
        }
        assert!(Instant::now() < deadline, "the worker reads its task: {screen:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    let read: String = screen.concat();
    assert!(
        read.contains("worker read: Fix the redirect and report"),
        "the task of the worker reaches its terminal as one line: {screen:?}"
    );
    assert!(
        read.contains("When the task is done, or when you cannot continue,")
            && read.contains("call the block_panel tool with operation worker_report"),
        "the sentence of the report follows the task: {screen:?}"
    );
    assert!(
        !app.agent_opens.panels.holds_prompts(),
        "the line of a worker goes out one time"
    );
    let _ = REPORT_SENTENCE;
}

#[test]
fn a_fifth_worker_of_one_orchestrator_is_refused() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let local_id = running(&mut app, 0);
    app.orchestrator_dock.allow_worker_program(&local_id, &program);
    for index in 0..MAX_WORKERS {
        let outcome = start(&mut app, &ctx, 0, &program, &format!("task {index}"));
        assert!(matches!(outcome, Outcome::WorkerStarted { .. }), "{outcome:?}");
    }
    let panels = app.board.panels.len();

    let refusal = start(&mut app, &ctx, 0, &program, "one more");

    assert_eq!(code(&refusal), "too_many_workers", "{refusal:?}");
    assert_eq!(app.board.panels.len(), panels, "a fifth worker opens no panel");
    assert_eq!(
        rows_of(&app, &local_id).len(),
        MAX_WORKERS,
        "the conversation names the four workers of the bound"
    );
}

#[test]
fn a_program_outside_the_chip_starts_nothing_and_writes_the_row_of_the_request() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let local_id = running(&mut app, 0);
    let panels = app.board.panels.len();

    let outcome = start(&mut app, &ctx, 0, &program, "Say hello and report.\nThen wait.");

    let Outcome::WorkerAsked {
        request,
        program: named,
        text,
    } = outcome
    else {
        panic!("a program outside the chip asks the person: {outcome:?}");
    };
    assert_eq!(named, program);
    assert!(
        text.starts_with("The person decides. You get one message when they do."),
        "the answer is the words of Horizon: {text}"
    );
    assert_eq!(app.board.panels.len(), panels, "a request starts nothing");
    assert_eq!(
        rows_of(&app, &local_id),
        vec![Entry::WorkerAsk {
            id: request,
            program: program.clone(),
            task: "Say hello and report.".to_owned(),
            state: WorkerAskState::Open,
        }],
        "the conversation shows one row with the first line of the task"
    );
    assert!(
        app.orchestrator_dock.workers_allowed(&local_id).is_empty(),
        "the request adds no program to the chip"
    );

    // The person answers `Not now`, and a second request of the same program asks
    // nothing again: it is refused with `not_allowed` and starts nothing.
    let request_id = match rows_of(&app, &local_id).pop() {
        Some(Entry::WorkerAsk { id, .. }) => id,
        other => panic!("the row of the request: {other:?}"),
    };
    app.press_worker_ask(
        &ctx,
        &local_id,
        &request_id,
        crate::app::block_requests::workers::WorkerAnswer::NotNow,
    );
    let again = start(&mut app, &ctx, 0, &program, "Say hello and report.");
    assert_eq!(code(&again), "not_allowed", "{again:?}");
    assert_eq!(app.board.panels.len(), panels, "a refusal starts no panel");
    assert_eq!(
        rows_of(&app, &local_id).len(),
        1,
        "the conversation holds the row that the person answered, and no second row"
    );
}

#[test]
fn the_workers_of_an_orchestrator_name_their_state_and_their_last_report() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let local_id = running(&mut app, 0);
    app.orchestrator_dock.allow_worker_program(&local_id, &program);
    let Outcome::WorkerStarted { worker, .. } = start(&mut app, &ctx, 0, &program, "fix it") else {
        panic!("the worker of the chip starts");
    };

    let read = app.apply_block_request(&orchestrator_request(&app, 0, Operation::Workers), &ctx);
    let Outcome::Workers { workers } = read else {
        panic!("the read of the workers answers them: {read:?}");
    };
    assert_eq!(
        workers,
        vec![WorkerSummary {
            worker: worker.clone(),
            program: program.clone(),
            kind: "claude".to_owned(),
            state: "idle".to_owned(),
            last_report: None,
        }],
        "the worker of the conversation is idle and reported nothing yet"
    );

    let reported = app.apply_block_request(
        &panel_request(
            &worker,
            Operation::WorkerReport {
                text: "the redirect is fixed".into(),
            },
        ),
        &ctx,
    );
    assert!(matches!(reported, Outcome::WorkerReported { .. }), "{reported:?}");
    let Outcome::Workers { workers } =
        app.apply_block_request(&orchestrator_request(&app, 0, Operation::Workers), &ctx)
    else {
        panic!("the read of the workers answers them");
    };
    assert_eq!(
        workers.first().and_then(|worker| worker.last_report.as_deref()),
        Some("the redirect is fixed"),
        "the read names the last report of the worker"
    );

    // A panel that the person closes leaves the record of the workers, on the pass
    // of the dock that reads the board.
    let id = app
        .board
        .panel_id_by_local_id(&worker)
        .expect("the panel of the worker");
    let _ = app.close_panel_returning_teardown(id);
    app.poll_orchestrator_dock(&ctx);
    let Outcome::Workers { workers } =
        app.apply_block_request(&orchestrator_request(&app, 0, Operation::Workers), &ctx)
    else {
        panic!("the read of the workers answers them");
    };
    assert!(
        workers.is_empty(),
        "a worker whose panel left the board leaves the list: {workers:?}"
    );
    let sent = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker: worker.clone(),
                text: "next".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&sent), "not_found", "a worker that left is not found");
}

#[test]
fn a_message_goes_to_one_worker_of_this_orchestrator_at_most_every_ten_seconds() {
    let (temp, ctx, mut app) = app();
    let program = with_preset(&mut app, temp.path());
    let local_id = running(&mut app, 0);
    app.orchestrator_dock.allow_worker_program(&local_id, &program);
    let Outcome::WorkerStarted { worker, .. } = start(&mut app, &ctx, 0, &program, "fix it") else {
        panic!("the worker of the chip starts");
    };

    // A worker of another conversation of this dock is not found here.
    let other = running(&mut app, 1);
    let stranger = app.apply_block_request(
        &orchestrator_request(
            &app,
            1,
            Operation::WorkerMessage {
                worker: worker.clone(),
                text: "next".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&stranger), "not_found", "another orchestrator has no such worker");
    assert_eq!(
        app.orchestrator_dock.workers_of(&other).len(),
        0,
        "another orchestrator has no worker of this one"
    );

    let unknown = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker: "11111111-2222-4333-8444-555555555555".into(),
                text: "next".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&unknown), "not_found");

    let sent = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker: worker.clone(),
                text: "start with the test".into(),
            },
        ),
        &ctx,
    );
    assert!(
        matches!(
            &sent,
            Outcome::WorkerMessage { worker: answered, program: answered_program }
                if *answered == worker && *answered_program == program
        ),
        "the answer names the worker and its program: {sent:?}"
    );
    assert!(
        entries_of(&app, &local_id).iter().any(|entry| *entry
            == Entry::WorkerMessage {
                panel: worker.clone(),
                program: program.clone(),
                text: "start with the test".to_owned(),
            }),
        "the conversation of the orchestrator shows the message"
    );
    assert!(
        app.agent_opens.panels.holds_prompts(),
        "the message waits for the program of the worker"
    );

    let too_soon = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker: worker.clone(),
                text: "and again".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&too_soon), "too_soon");

    // The interval of the bound passes, and the next message goes out.
    app.orchestrator_dock
        .rewind_worker_message(&local_id, &worker, Duration::from_millis(WORKER_INTERVAL_MILLIS));
    let after = app.apply_block_request(
        &orchestrator_request(
            &app,
            0,
            Operation::WorkerMessage {
                worker,
                text: "and again".into(),
            },
        ),
        &ctx,
    );
    assert!(matches!(after, Outcome::WorkerMessage { .. }), "{after:?}");
}

#[test]
fn a_report_reaches_the_orchestrator_as_one_turn_after_a_busy_turn_ends() {
    let (temp, ctx, mut app) = app();
    let lines = temp.path().join("lines.txt");
    let body = ORCHESTRATOR
        .replace("@LINES@", &lines.display().to_string())
        .replace("@SESSION@", SESSION);
    let stand_in = horizon_core::stand_in_program_in(temp.path(), "orchestrator-stand-in.sh", &body)
        .expect("the stand-in program of the orchestrator");
    app.orchestrator_dock.set_program(stand_in);
    let local_id = workspace_local_id(&app, 0);
    assert!(
        app.say_to_orchestrator(&ctx, &local_id, "hello"),
        "the message of the person reaches the conversation"
    );
    wait_for(&mut app, "the program of the orchestrator runs", |app| {
        app.orchestrator_dock.running(&local_id) && !app.orchestrator_dock.busy(&local_id)
    });
    let program = with_preset(&mut app, temp.path());
    app.orchestrator_dock.allow_worker_program(&local_id, &program);
    let Outcome::WorkerStarted { worker, .. } = start(&mut app, &ctx, 0, &program, "fix it") else {
        panic!("the worker of the chip starts");
    };

    // A worker that is no worker of this orchestrator is refused.
    let stranger = app.apply_block_request(
        &panel_request(
            &app.board.panels[0].local_id.clone(),
            Operation::WorkerReport { text: "done".into() },
        ),
        &ctx,
    );
    assert_eq!(code(&stranger), "caller_unavailable");

    let reported = app.apply_block_request(
        &panel_request(
            &worker,
            Operation::WorkerReport {
                text: "the redirect is fixed".into(),
            },
        ),
        &ctx,
    );
    assert!(matches!(reported, Outcome::WorkerReported { .. }), "{reported:?}");
    wait_for(&mut app, "the report reaches the program", |_| {
        lines_of(&lines)
            .iter()
            .any(|line| line.contains("Report of your worker"))
    });
    let report = lines_of(&lines)
        .into_iter()
        .find(|line| line.contains("Report of your worker"))
        .expect("the line of the report");
    assert!(
        report.contains(&format!("({worker}):")) && report.contains("the redirect is fixed"),
        "the turn names the program, the panel and the text: {report}"
    );
    assert!(
        entries_of(&app, &local_id).iter().any(|entry| *entry
            == Entry::WorkerReport {
                panel: worker.clone(),
                program: program.clone(),
                text: "the redirect is fixed".to_owned(),
            }),
        "the conversation of the orchestrator shows the report"
    );

    // One report of one worker every ten seconds.
    let too_soon = app.apply_block_request(
        &panel_request(
            &worker,
            Operation::WorkerReport {
                text: "one more".into(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&too_soon), "too_soon");

    // Eight reports wait for the end of a turn, and the ninth is refused with
    // `busy`. The turn of this frame works, so no line of them goes out yet.
    app.orchestrator_dock.begin_turn_for_test(&local_id);
    let refused = fill_report_queue(&mut app, &ctx, &local_id, &worker);
    assert_eq!(refused.0, MAX_WAITING_REPORTS, "the ninth report that waits is refused");
    assert_eq!(refused.1, "busy", "the refusal names the queue of the reports");
    assert!(
        !lines_of(&lines).iter().any(|line| line.contains("report 0")),
        "no report goes out while the turn of the orchestrator runs"
    );

    app.orchestrator_dock.end_turn_for_test(&local_id);
    wait_for(&mut app, "the first report waits for the turn", |_| {
        lines_of(&lines).iter().any(|line| line.contains("report 0"))
    });
}

/// Sends reports of the worker `worker` until the queue is full, and answers the
/// index and the code of the refusal of the ninth.
fn fill_report_queue(app: &mut HorizonApp, ctx: &Context, local_id: &str, worker: &str) -> (usize, String) {
    for index in 0..=MAX_WAITING_REPORTS {
        app.orchestrator_dock
            .rewind_worker_report(local_id, worker, Duration::from_millis(WORKER_INTERVAL_MILLIS));
        let outcome = app.apply_block_request(
            &panel_request(
                worker,
                Operation::WorkerReport {
                    text: format!("report {index}"),
                },
            ),
            ctx,
        );
        if let Outcome::Failed { code, .. } = outcome {
            return (index, code);
        }
    }
    panic!("the report of a full queue is refused");
}

/// The lines that the program of the orchestrator read, in order.
fn lines_of(lines: &Path) -> Vec<String> {
    fs::read_to_string(lines)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}
