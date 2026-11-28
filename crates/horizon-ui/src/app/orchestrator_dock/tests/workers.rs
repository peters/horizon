//! The rows of the workers of an orchestrator: the row of a request that the person
//! decides, the reference row of a worker, its messages and its reports, and the
//! chip `Workers` that holds the programs of the settings.
//!
//! The program of a conversation of these tests is a `/bin/sh` script that the test
//! writes into a folder of its own: it names its session, records each line that it
//! reads in one file and answers one text. No test starts an agent program.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::agent_bridge::{Entry, WorkerAskState};
use horizon_core::{PresetConfig, WorkspaceState};

use crate::app::block_requests::workers::WORKER_MARK;
use crate::app::orchestrator_dock::list;
use crate::app::orchestrator_dock::proposal;
use crate::app::orchestrator_dock::tests::chat;
use crate::app::orchestrator_dock::tests::frame::{Drawn, Window, texts};
use crate::app::orchestrator_dock::tests::front_desk::WIDE;
use crate::app::orchestrator_dock::worker_rows;
use crate::app::orchestrator_dock::workers_menu::NONE;
use crate::app::sidebar::menus::CHECK_MARK;
use crate::app::test_support::editor_workspace_state;

/// The workspace of these tests.
const WORKSPACE: &str = "0f1e2d3c-4b5a-4697-8877-665544332211";
/// The session that the stand-in names.
const SESSION: &str = "00000000-0000-4000-8000-000000000001";

/// The stand-in of the orchestrator: it records each line that it reads in
/// `@LINES@` and answers one text and one result.
const ORCHESTRATOR: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
while IFS= read -r line; do
  printf '%s\n' "$line" >> '@LINES@'
  printf '%s\n' '{"type":"assistant","message":{"id":"m-1","content":[{"type":"text","text":"the orchestrator answers"}]}}'
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
done
"#;

/// The stand-in of a worker: it prints one line and answers each line that it reads.
const WORKER: &str =
    "#!/bin/sh\nprintf 'worker ready\\n'\nwhile IFS= read -r line; do printf 'worker read: %s\\n' \"$line\"; done";

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// One press of the person on `rect`.
fn press(window: &mut Window, rect: Rect) {
    let at = rect.center();
    window.frame(vec![Event::PointerMoved(at), button(at, true)]);
    window.frame(vec![button(at, false)]);
}

/// The one text of `drawn` that reads `text`.
fn drawn_text(drawn: &[Drawn], text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text)
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws {text:?}: {drawn:?}"))
}

/// Whether `drawn` holds the text `text`.
fn draws(drawn: &[Drawn], text: &str) -> bool {
    drawn.iter().any(|drawn| drawn.text == text)
}

/// The preset `Worker Codex` of these tests: a preset of an agent kind whose program
/// is the stand-in of `folder`.
fn worker_preset(folder: &Path) -> PresetConfig {
    let program = horizon_core::stand_in_program_in(folder, "worker-stand-in", WORKER).expect("the stand-in program");
    PresetConfig {
        name: "Worker Codex".to_owned(),
        alias: None,
        kind: horizon_core::PanelKind::Claude,
        command: Some(program.display().to_string()),
        args: Vec::new(),
        resume: horizon_core::PanelResume::Fresh,
        ssh_connection: None,
    }
}

/// A window whose dock shows the conversation of `WORKSPACE`, whose program is the
/// stand-in of `program`.
fn window(program: &Path, preset: Option<PresetConfig>) -> Window {
    let workspace: WorkspaceState = editor_workspace_state(WORKSPACE, [40.0, 40.0]);
    let mut window = Window::with_program(WIDE, vec![workspace], program);
    window.app().orchestrator_dock.front_desk_mut().show_workspace();
    window.focus_workspace(WORKSPACE);
    if let Some(preset) = preset {
        window.app().template_config.presets.push(preset);
    }
    let _ = window.frame(Vec::new());
    window
}

/// The stand-in of the program of the orchestrator of a test, with the file that
/// records the lines that it reads.
fn orchestrator_stand_in(folder: &Path) -> (PathBuf, PathBuf) {
    let lines = folder.join("lines.txt");
    let body = ORCHESTRATOR
        .replace("@LINES@", &lines.display().to_string())
        .replace("@SESSION@", SESSION);
    (chat::stand_in(folder, &body), lines)
}

/// Starts the program of the conversation of `WORKSPACE` with one message, and waits
/// until it runs and answers.
fn start_orchestrator(window: &mut Window) {
    chat::send(window, "hello");
    window.wait_for("the program of the orchestrator", |app| {
        app.orchestrator_dock.running(WORKSPACE) && !app.orchestrator_dock.busy(WORKSPACE)
    });
}

/// The lines that the program of the orchestrator read.
fn lines_of(lines: &Path) -> Vec<String> {
    fs::read_to_string(lines)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Waits until the program of the orchestrator read a line that holds `text`.
fn wait_line(window: &mut Window, lines: &Path, text: &str) {
    window.wait_for(text, |_| lines_of(lines).iter().any(|line| line.contains(text)));
}

/// The row of a request for a worker, added to the conversation of the workspace.
fn request(window: &mut Window, id: &str) {
    window
        .app()
        .orchestrator_dock
        .worker_ask(WORKSPACE, id, "Worker Codex", "Fix the redirect\nand report");
}

#[test]
fn the_row_of_a_request_draws_its_caption_its_program_its_task_and_three_controls() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (program, _lines) = orchestrator_stand_in(folder.path());
    let mut window = window(&program, None);
    request(&mut window, "r-1");
    let drawn = texts(&window.frame(Vec::new()));
    let body = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .body;

    let caption = drawn_text(&drawn, worker_rows::CAPTION);
    let program_name = drawn_text(&drawn, "Worker Codex");
    let task = drawn_text(&drawn, "Fix the redirect");
    let start = drawn_text(&drawn, worker_rows::START);
    let always = drawn_text(&drawn, worker_rows::ALWAYS);
    let not_now = drawn_text(&drawn, worker_rows::NOT_NOW);

    // The row stands at the left of the list, 13 points inside it, and its text 12
    // points inside the accent box.
    let left = body.min.x + list::LIST_PADDING + list::CARD_INDENT + proposal::PADDING;
    assert!(
        (caption.rect.min.x - left).abs() < 0.5,
        "the caption stands at the padding of the box: {} against {left}",
        caption.rect.min.x
    );
    assert!(
        caption.rect.max.y <= program_name.rect.min.y,
        "the caption stands above the name of the program"
    );
    assert!(
        program_name.rect.max.y <= task.rect.min.y,
        "the name of the program stands above the task"
    );
    assert!(task.rect.max.y <= start.rect.min.y, "the controls stand under the task");
    // The three controls stand on one line, 8 points apart, and each of them is a
    // label of 12 points inside a control of 28 points.
    assert_eq!(start.rect.center().y, always.rect.center().y);
    assert_eq!(always.rect.center().y, not_now.rect.center().y);
    assert!(
        (always.rect.min.x - start.rect.max.x - 2.0 * proposal::BUTTON_PADDING - proposal::BUTTON_GAP).abs() < 0.5,
        "12 points of padding and 8 points of air stand between two labels"
    );
    assert!(
        start.rect.height() < proposal::BUTTON_HEIGHT,
        "a label of 12 points stands inside a control of 28 points"
    );
    assert!(
        not_now.rect.max.x <= body.max.x,
        "the three controls stay inside the list"
    );
}

#[test]
fn a_press_on_start_starts_one_worker_and_tells_the_program() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (program, lines) = orchestrator_stand_in(folder.path());
    let mut window = window(&program, Some(worker_preset(folder.path())));
    start_orchestrator(&mut window);
    request(&mut window, "r-1");
    let start = drawn_text(&texts(&window.frame(Vec::new())), worker_rows::START);
    let panels = window.app().board.panels.len();

    press(&mut window, start.rect);

    assert_eq!(
        window.app().board.panels.len(),
        panels + 1,
        "the press starts one worker"
    );
    let drawn = texts(&window.frame(Vec::new()));
    assert!(
        draws(&drawn, worker_rows::STARTED),
        "the row of the request says Started: {drawn:?}"
    );
    assert!(
        !draws(&drawn, worker_rows::START),
        "a request that the person answered takes no press: {drawn:?}"
    );
    let worker = window
        .app()
        .board
        .panels
        .last()
        .map(|panel| panel.local_id.clone())
        .expect("the worker of the request");
    assert!(
        window
            .app()
            .orchestrator_dock
            .conversation_entries(WORKSPACE)
            .iter()
            .any(|entry| matches!(entry, Entry::WorkerStarted { panel, .. } if *panel == worker)),
        "the conversation names the worker that the press started"
    );
    // The mark of the worker is Horizon's own text on the panel, which the titlebar
    // draws as the chip `opened by <mark>` while that chip fits beside the badges of
    // the panel.
    assert_eq!(
        window
            .app()
            .board
            .panels
            .last()
            .and_then(horizon_core::Panel::opened_by),
        Some(WORKER_MARK),
        "the panel of a worker carries the mark of the orchestrator"
    );
    wait_line(&mut window, &lines, "The person started the worker of request r-1.");
    assert!(
        window.app().orchestrator_dock.workers_allowed(WORKSPACE).is_empty(),
        "a press on Start starts one worker and adds no program to the chip"
    );
}

#[test]
fn a_press_on_always_adds_the_program_to_the_chip() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (program, lines) = orchestrator_stand_in(folder.path());
    let mut window = window(&program, Some(worker_preset(folder.path())));
    start_orchestrator(&mut window);
    request(&mut window, "r-2");
    let always = drawn_text(&texts(&window.frame(Vec::new())), worker_rows::ALWAYS);

    press(&mut window, always.rect);

    assert_eq!(
        window.app().orchestrator_dock.workers_allowed(WORKSPACE),
        ["Worker Codex"],
        "Always for this work item adds the program to the chip"
    );
    let drawn = texts(&window.frame(Vec::new()));
    assert!(
        draws(&drawn, "Started; Worker Codex is allowed for this work item"),
        "the row says what the press did: {drawn:?}"
    );
    wait_line(&mut window, &lines, "The person started the worker of request r-2.");
    assert!(
        window
            .app()
            .orchestrator_dock
            .conversation_entries(WORKSPACE)
            .iter()
            .any(|entry| matches!(
                entry,
                Entry::WorkerAsk { id, state: WorkerAskState::Always, .. } if id == "r-2"
            )),
        "the row of the request keeps the answer of the person"
    );
}

#[test]
fn a_press_on_not_now_starts_nothing_and_tells_the_program() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (program, lines) = orchestrator_stand_in(folder.path());
    let mut window = window(&program, Some(worker_preset(folder.path())));
    start_orchestrator(&mut window);
    request(&mut window, "r-3");
    let not_now = drawn_text(&texts(&window.frame(Vec::new())), worker_rows::NOT_NOW);
    let panels = window.app().board.panels.len();

    press(&mut window, not_now.rect);

    assert_eq!(window.app().board.panels.len(), panels, "Not now starts nothing");
    let drawn = texts(&window.frame(Vec::new()));
    assert!(
        draws(&drawn, worker_rows::NOT_STARTED),
        "the row of the request says Not started: {drawn:?}"
    );
    wait_line(
        &mut window,
        &lines,
        "The person did not start the worker of request r-3.",
    );
    assert!(
        window.app().orchestrator_dock.workers_allowed(WORKSPACE).is_empty(),
        "Not now adds no program to the chip"
    );
}

#[test]
fn a_request_that_a_restart_loaded_as_open_takes_no_press() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (program, _lines) = orchestrator_stand_in(folder.path());
    let mut window = window(&program, Some(worker_preset(folder.path())));
    window.app().orchestrator_dock.load_for_test(
        WORKSPACE,
        vec![Entry::WorkerAsk {
            id: "loaded".to_owned(),
            program: "Worker Codex".to_owned(),
            task: "Say hello".to_owned(),
            state: WorkerAskState::Open,
        }],
    );
    let panels = window.app().board.panels.len();

    let drawn = texts(&window.frame(Vec::new()));

    assert!(
        draws(&drawn, worker_rows::NOT_STARTED),
        "a request of an earlier run reads Not started: {drawn:?}"
    );
    for control in [worker_rows::START, worker_rows::ALWAYS, worker_rows::NOT_NOW] {
        assert!(!draws(&drawn, control), "a loaded row draws no control: {drawn:?}");
    }
    assert_eq!(window.app().board.panels.len(), panels, "a loaded row starts nothing");
}

#[test]
fn the_chip_of_the_workers_adds_and_removes_a_program_and_the_record_keeps_it() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let (program, _lines) = orchestrator_stand_in(folder.path());
    let mut window = window(&program, Some(worker_preset(folder.path())));
    // The session of the test keeps the conversations of its workspaces, as a saved
    // start does, so the record of a conversation is written.
    window
        .app()
        .orchestrator_dock
        .set_storage(Some(folder.path().join("conversations")));
    // The conversation of the workspace holds a session of this run, so the record
    // of the conversation is written.
    window.app().orchestrator_dock.load_for_test(WORKSPACE, Vec::new());
    let index = window.app().template_config.presets.len() - 1;
    // The search of the programs of this run reads the folders of the `PATH` on the
    // coordination worker, so the frame waits for its answer.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !window.app().orchestrator_dock.worker_program_found(index) {
        assert!(Instant::now() < deadline, "the search of the programs answers");
        let _ = window.frame(Vec::new());
        window.app().settle_browser_host_io();
        std::thread::sleep(Duration::from_millis(5));
    }
    let drawn = texts(&window.frame(Vec::new()));
    let value = drawn_text(&drawn, NONE);
    assert!(draws(&drawn, "Workers"), "the composer keeps its chip of the workers");
    let panels = window.app().board.panels.len();

    // A press on the chip opens the menu of the programs.
    press(&mut window, value.rect);
    let drawn = texts(&window.frame(Vec::new()));
    let row = drawn_text(&drawn, "Worker Codex");

    // A press on a row adds that program and keeps the menu open.
    press(&mut window, row.rect);
    let drawn = texts(&window.frame(Vec::new()));
    assert!(
        draws(&drawn, format!("{CHECK_MARK} Worker Codex").as_str()),
        "the row of a program that the chip holds carries a check mark: {drawn:?}"
    );
    assert!(
        draws(&drawn, "Worker Codex"),
        "the menu stays open after a press, so the person chooses two programs"
    );
    assert_eq!(
        window.app().orchestrator_dock.workers_allowed(WORKSPACE),
        ["Worker Codex"]
    );

    // The record of the conversation of the workspace keeps the chip, so a later
    // run reads the programs that the person allowed.
    let mut board = std::mem::take(&mut window.app().board);
    assert!(
        window
            .app()
            .orchestrator_dock
            .sync_conversation_records(&mut board, "claude"),
        "the record of the conversation follows the chip"
    );
    window.app().board = board;
    let record = window
        .app()
        .board
        .workspaces
        .iter()
        .find(|workspace| workspace.local_id == WORKSPACE)
        .and_then(|workspace| workspace.orchestrator.clone());
    assert_eq!(
        record.map(|record| record.allowed_workers),
        Some(vec!["Worker Codex".to_owned()]),
        "the chip of the workers is stored with the conversation"
    );

    // A second press on the row that carries the check mark removes the program, and
    // the chip reads `not chosen` again.
    let drawn = texts(&window.frame(Vec::new()));
    let row = drawn_text(&drawn, &format!("{CHECK_MARK} Worker Codex"));
    press(&mut window, row.rect);
    window.frame(Vec::new());
    assert!(
        window.app().orchestrator_dock.workers_allowed(WORKSPACE).is_empty(),
        "a second press removes the program"
    );
    let drawn = texts(&window.frame(Vec::new()));
    assert!(
        draws(&drawn, NONE),
        "a chip that holds no program reads not chosen: {drawn:?}"
    );
    assert_eq!(window.app().board.panels.len(), panels, "the chip starts no panel");
    assert!(
        window
            .app()
            .orchestrator_dock
            .last_frame()
            .is_some_and(|frame| frame.chips.width() > 0.0),
        "the row of the chips stands in the composer"
    );
}
