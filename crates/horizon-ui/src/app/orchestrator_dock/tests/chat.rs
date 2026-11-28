//! The conversation of the dock on a pipe: a message of the person, the answer
//! of a program that runs with no terminal, the stop control, a program that
//! does not start, two workspaces, and the end of Horizon.
//!
//! The stand-in of every test is a `/bin/sh` script that the test writes into a
//! folder of its own, as the tests of `horizon-core::agent_bridge` do. It prints
//! hand-written JSON lines of the Claude Code format and reads the lines of the
//! person, so no test starts an agent program and no test reaches the network.
//! The path of the stand-in reaches the dock through the seam of
//! `HORIZON_ORCHESTRATOR_PROGRAM`, never through the environment of the test
//! process.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton, Pos2};
use horizon_core::agent_bridge::{Entry, ToolState};

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::composer::FIELD_ID;
use crate::app::orchestrator_dock::session::{LIMIT_NOTICE, Phase, STOPPED_NOTICE, Session, TOO_LONG_NOTICE};
use crate::app::orchestrator_dock::tests::frame::{Drawn, Window, texts};
use crate::app::test_support::editor_workspace_state;

/// The window of these tasks: wide enough that the widest dock stands beside the
/// canvas.
pub(super) const WIDE: [f32; 2] = [2000.0, 1000.0];
/// The session that every stand-in names.
pub(super) const SESSION: &str = "00000000-0000-4000-8000-000000000001";

/// The stand-in of an agent that answers each line of the person with one text
/// and one result, and names the cost of that turn.
const ANSWERING: &str = r#"echo $$ > '@PID@'
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
@TOOL@  printf '{"type":"assistant","message":{"id":"msg-%s","content":[{"type":"text","text":"the stand-in answers"}]}}\n' "$n"
  printf '%s\n' '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","isUsingOverage":@OVERAGE@}}'
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5,"model_usage_since_last_result":{"stand-in":{"costUSD":@COST@}}}'
done
"#;

/// The tool call of one turn, which a stand-in prints before the text of that
/// turn: the agent calls `Bash`, and its result arrives.
const TOOL_CALL: &str = r#"  printf '{"type":"assistant","message":{"id":"tools-%s","content":[{"type":"tool_use","id":"toolu-%s","name":"Bash","input":{"command":"echo stand-in"}}]}}\n' "$n" "$n"
  printf '{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu-%s","type":"tool_result","content":"stand-in","is_error":false}]}}\n' "$n"
"#;

/// The tool call of a turn that fails, which a stand-in prints in place of
/// [`TOOL_CALL`]: the agent calls `Bash` with a command that ends in an error, and
/// the result says why on two lines.
const FAILING_TOOL_CALL: &str = r#"  printf '{"type":"assistant","message":{"id":"tools-%s","content":[{"type":"tool_use","id":"toolu-%s","name":"Bash","input":{"command":"exit 3"}}]}}\n' "$n" "$n"
  printf '{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu-%s","type":"tool_result","content":"the command failed\\nsecond line","is_error":true}]}}\n' "$n"
"#;

/// The stand-in of a program that names its session and then answers nothing.
const SILENT: &str = r#"echo $$ > '@PID@'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
while IFS= read -r line; do :; done
"#;

/// The stand-in of a program that ignores the close of its input and the
/// termination signal: only the kill signal ends it.
pub(super) const STUBBORN: &str = r#"trap '' TERM
echo $$ > '@PID@'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
while :; do sleep 0.05; done
"#;

/// One stand-in program of the dock, written as a program of `folder`.
pub(super) fn stand_in(folder: &Path, body: &str) -> PathBuf {
    stand_in_named(folder, "stand-in.sh", body)
}

/// The stand-in program `name` of the dock, written as a program of `folder`, so
/// one folder holds the programs of two conversations.
pub(super) fn stand_in_named(folder: &Path, name: &str, body: &str) -> PathBuf {
    horizon_core::stand_in_program_in(folder, name, body).expect("the stand-in program of the test")
}

/// The body of the stubborn stand-in of `pid`.
pub(super) fn stubborn(pid: &Path) -> String {
    STUBBORN
        .replace("@PID@", &pid.display().to_string())
        .replace("@SESSION@", SESSION)
}

/// The status that the frames of a close ask the process to end with, and the
/// frames that run until the window asks for it. A test build records the status
/// where a normal build ends the process.
pub(super) fn exit_status(window: &mut Window) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = window.app().exit_code {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "the close of Horizon never asked for the exit"
        );
        let _ = window.frame(Vec::new());
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The body of a stand-in that answers every line, with `tool` tool calls, an
/// overage state of `overage` and the cost of `cost` for each turn.
pub(super) fn answering(pid: &Path, tool: bool, overage: bool, cost: f64) -> String {
    answering_with(pid, if tool { TOOL_CALL } else { "" }, overage, cost)
}

/// The body of a stand-in that answers every line, with the lines of `tool` in
/// place of its tool call, an overage state of `overage` and the cost of `cost`
/// for each turn.
fn answering_with(pid: &Path, tool: &str, overage: bool, cost: f64) -> String {
    ANSWERING
        .replace("@PID@", &pid.display().to_string())
        .replace("@SESSION@", SESSION)
        .replace("@TOOL@", tool)
        .replace("@OVERAGE@", if overage { "true" } else { "false" })
        .replace("@COST@", &format!("{cost}"))
}

/// The body of a stand-in that never answers.
fn silent(pid: &Path) -> String {
    SILENT
        .replace("@PID@", &pid.display().to_string())
        .replace("@SESSION@", SESSION)
}

/// A window on one workspace `chat` whose orchestrator runs `program`.
pub(super) fn window_of(program: &Path) -> Window {
    let mut window = Window::with_program(WIDE, vec![editor_workspace_state("chat", [40.0, 40.0])], program);
    window.focus_workspace("chat");
    let _ = window.frame(Vec::new());
    window
}

fn pointer(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// The entries of the conversation of `local_id`.
pub(super) fn entries_of(app: &HorizonApp, local_id: &str) -> Vec<Entry> {
    app.orchestrator_dock
        .session(local_id)
        .map(|session| session.snapshot().entries.clone())
        .unwrap_or_default()
}

/// The state of the conversation of `local_id`, which the chip of the header
/// names.
pub(super) fn state_of(app: &HorizonApp, local_id: &str) -> Option<Phase> {
    app.orchestrator_dock.session(local_id).map(Session::phase)
}

/// Clicks the field of the dock, which gives it the keyboard.
fn click_field(window: &mut Window) {
    let field = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .field;
    let at = field.center();
    let _ = window.frame(vec![Event::PointerMoved(at), pointer(at, true)]);
    let _ = window.frame(vec![pointer(at, false)]);
    assert!(
        window
            .ctx()
            .memory(|memory| memory.has_focus(egui::Id::unique(FIELD_ID))),
        "the field of the dock holds the keyboard after the click"
    );
}

/// Types `text` into the field that holds the keyboard.
fn type_text(window: &mut Window, text: &str) {
    let _ = window.frame(vec![Event::Text(text.to_owned())]);
}

/// Presses `key` with `modifiers` on the field that holds the keyboard.
fn press(window: &mut Window, key: Key, modifiers: Modifiers) {
    let _ = window.frame(vec![Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    }]);
}

/// Writes `text` in the field of the dock and sends it with Enter.
pub(super) fn send(window: &mut Window, text: &str) {
    click_field(window);
    type_text(window, text);
    press(window, Key::Enter, Modifiers::NONE);
}

/// Presses the control of the composer, which is the send or the stop control of
/// this frame.
fn press_control(window: &mut Window) {
    let control = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .send;
    let at = control.center();
    let _ = window.frame(vec![Event::PointerMoved(at), pointer(at, true)]);
    let _ = window.frame(vec![pointer(at, false)]);
}

/// Waits until the process `pid` is gone, and names what the test waited for.
#[cfg(target_os = "linux")]
pub(super) fn wait_until_no_process(what: &str, pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Path::new(&format!("/proc/{pid}")).exists() {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The process id that the stand-in of a test wrote into `pid`.
pub(super) fn child_of(pid: &Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(pid)
            && let Ok(child) = text.trim().parse::<u32>()
        {
            return child;
        }
        assert!(Instant::now() < deadline, "the test waited for the child of a stand-in");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The texts of one frame of `window`.
fn drawn(window: &mut Window) -> Vec<Drawn> {
    let output = window.frame(Vec::new());
    texts(&output)
}

/// The one text of `drawn` that reads `text`.
fn drawn_text(drawn: &[Drawn], text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text)
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws {text:?}: {drawn:?}"))
}

#[test]
fn enter_sends_the_draft_and_shift_enter_makes_a_new_line() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    click_field(&mut window);
    type_text(&mut window, "hello there");
    press(&mut window, Key::Enter, Modifiers::SHIFT);

    // Shift+Enter keeps the draft and adds one line to it.
    assert_eq!(
        window.app().orchestrator_dock.draft("chat"),
        "hello there\n",
        "the draft of the person after Shift+Enter"
    );
    assert!(entries_of(window.app(), "chat").is_empty(), "no send yet");

    press(&mut window, Key::Enter, Modifiers::NONE);

    // Enter sends the message, clears the field and asks for the bridge.
    assert_eq!(window.app().orchestrator_dock.draft("chat"), "");
    assert_eq!(
        entries_of(window.app(), "chat"),
        vec![Entry::Person {
            text: "hello there".to_owned(),
            via_front_desk: false
        }],
        "the message of the person, without the new line that it carried"
    );
    assert_eq!(
        state_of(window.app(), "chat"),
        Some(Phase::Starting),
        "the start of the bridge waits on the worker"
    );

    window.wait_for("the answer of the stand-in", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
            && !app
                .orchestrator_dock
                .session("chat")
                .is_some_and(|session| session.snapshot().busy)
    });
    assert_eq!(state_of(window.app(), "chat"), Some(Phase::Running));
    let _ = pid;
}

#[test]
fn a_draft_of_white_space_sends_nothing() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    click_field(&mut window);
    type_text(&mut window, "   ");
    press(&mut window, Key::Enter, Modifiers::NONE);

    assert!(
        entries_of(window.app(), "chat").is_empty(),
        "a blank draft sends nothing"
    );
    assert_eq!(
        window.app().orchestrator_dock.draft("chat"),
        "   ",
        "the field keeps the white space of the person"
    );
}

#[test]
fn the_bubble_of_the_person_lies_at_the_right_and_the_text_of_the_agent_at_the_left() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, true, false, 0.01));
    let mut window = window_of(&program);

    send(&mut window, "hello there");
    window.wait_for("the answer with one tool call", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Tool { state, .. } if *state != ToolState::Running))
    });

    let dock = window
        .app()
        .orchestrator_dock
        .frame_rect()
        .expect("the dock of a wide window");
    let drawn = drawn(&mut window);
    let person = drawn_text(&drawn, "hello there");
    let agent = drawn_text(&drawn, "the stand-in answers");
    let tool = drawn_text(&drawn, "Bash");

    assert!(
        person.rect.min.x > dock.center().x,
        "the bubble of the person {:?} lies in the right half of the dock {dock:?}",
        person.rect
    );
    assert!(
        agent.rect.max.x < dock.center().x,
        "the text of the agent {:?} lies in the left half of the dock {dock:?}",
        agent.rect
    );
    assert!(
        person.rect.min.y < agent.rect.min.y,
        "the message of the person {:?} stands over the answer {:?}",
        person.rect,
        agent.rect
    );
    assert!(
        tool.rect.min.y > person.rect.max.y && tool.rect.max.y < agent.rect.min.y,
        "the row of the tool {:?} stands between the bubble {:?} and the answer {:?}",
        tool.rect,
        person.rect,
        agent.rect
    );
    let summary = drawn_text(&drawn, "echo stand-in");
    assert!(
        summary.rect.min.x > tool.rect.min.x,
        "the summary {:?} follows the name {:?}",
        summary.rect,
        tool.rect
    );
    assert!(
        !drawn.iter().any(|text| text.text == "stand-in"),
        "a tool that is done draws no line of its result"
    );
}

#[test]
fn a_failed_tool_keeps_its_command_and_draws_the_first_line_of_its_result() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering_with(&pid, FAILING_TOOL_CALL, false, 0.01));
    let mut window = window_of(&program);

    send(&mut window, "use a tool please");
    window.wait_for("the failed tool call", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Tool { state, .. } if *state == ToolState::Failed))
    });

    let entries = entries_of(window.app(), "chat");
    let Some(Entry::Tool {
        summary, result, state, ..
    }) = entries.iter().find(|entry| matches!(entry, Entry::Tool { .. }))
    else {
        panic!("the conversation holds the tool call: {entries:?}");
    };
    assert_eq!(*state, ToolState::Failed);
    assert_eq!(summary, "exit 3", "the row keeps the command that it was asked to run");
    assert_eq!(
        result.as_deref(),
        Some("the command failed"),
        "the row holds the first line of the result"
    );

    let drawn = drawn(&mut window);
    let command = drawn_text(&drawn, "exit 3");
    let line = drawn_text(&drawn, "the command failed");
    assert!(
        line.rect.min.y > command.rect.max.y,
        "the result {:?} stands under the command {:?}",
        line.rect,
        command.rect
    );
    assert!(
        line.rect.min.x >= command.rect.min.x - 0.5,
        "the result {:?} stands under the summary {:?}",
        line.rect,
        command.rect
    );
    assert_eq!(line.color, Some(crate::theme::PALETTE_RED()));
    assert!(
        !drawn.iter().any(|text| text.text == "second line"),
        "the row draws one line of the result"
    );
}

#[test]
fn a_program_that_does_not_start_leaves_a_notice_and_the_next_send_tries_again() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = folder.path().join("stand-in.sh");
    // A file without the permission to run is a program that does not start.
    std::fs::write(&program, "not a program of the dock\n").expect("the file of the test");
    let mut window = window_of(&program);

    send(&mut window, "hello there");
    window.wait_for("the notice of a program that does not start", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Notice { text } if text.starts_with("Claude Code did not start: ")))
    });
    let notice = entries_of(window.app(), "chat")
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Notice { text } if text.starts_with("Claude Code did not start: ") => Some(text),
            _ => None,
        })
        .expect("the notice of the failed start");
    assert!(
        notice.len() > "Claude Code did not start: ".len(),
        "the notice names the error of the system: {notice}"
    );
    assert_eq!(state_of(window.app(), "chat"), Some(Phase::Ended));

    // The same path is a program now, so the next send starts a bridge.
    let pid = folder.path().join("pid");
    stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    send(&mut window, "and again");
    window.wait_for("the answer of the second start", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    assert_eq!(state_of(window.app(), "chat"), Some(Phase::Running));
}

#[test]
fn the_frame_returns_at_once_while_the_start_of_the_child_is_held() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    // The worker of the dock holds the start until the test releases it.
    let (release, released) = mpsc::channel::<()>();
    let (started, holding) = mpsc::channel::<()>();
    let _ = window.app().orchestrator_dock.worker.write(move || {
        let _ = started.send(());
        let _ = released.recv();
    });
    holding
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker of the dock holds the start");
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        let _ = release.send(());
    });

    let started = Instant::now();
    send(&mut window, "hello there");
    let elapsed = started.elapsed();
    // The release thread runs before the first assertion, so the worker ends even
    // when an assertion below fails.
    let _ = &releasing;
    assert!(
        elapsed < Duration::from_secs(2),
        "the frame of the person took {elapsed:?} while the start waited"
    );
    assert_eq!(
        state_of(window.app(), "chat"),
        Some(Phase::Starting),
        "the start waits on the worker and the message waits with it"
    );
    assert!(
        !entries_of(window.app(), "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. })),
        "no answer arrived while the start waited"
    );

    window.wait_for("the answer after the held start", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    releasing.join().expect("the thread that releases the worker");
}

#[test]
fn the_stop_control_ends_a_turn_and_leaves_no_process() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &silent(&pid));
    let mut window = window_of(&program);

    send(&mut window, "a turn that never ends");
    let child = child_of(&pid);
    window.wait_for("the working state of the conversation", |app| {
        app.orchestrator_dock
            .session("chat")
            .is_some_and(|session| session.phase() == Phase::Running && session.busy())
    });

    press_control(&mut window);
    window.wait_for("the stopped conversation", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Notice { text } if text == STOPPED_NOTICE))
    });
    assert_eq!(state_of(window.app(), "chat"), Some(Phase::Stopped));
    assert!(
        !window
            .app()
            .orchestrator_dock
            .session("chat")
            .is_some_and(|session| session.snapshot().busy),
        "the stop ends the turn in flight"
    );
    #[cfg(target_os = "linux")]
    wait_until_no_process("the stop of the program", child);
    let _ = child;
}

#[test]
fn a_held_stop_never_holds_a_job_of_the_browser_worker() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &silent(&pid));
    let mut window = window_of(&program);

    send(&mut window, "a turn that never ends");
    let child = child_of(&pid);
    window.wait_for("the working state of the conversation", |app| {
        app.orchestrator_dock
            .session("chat")
            .is_some_and(|session| session.phase() == Phase::Running && session.busy())
    });

    // The worker of the dock holds the stop until the test releases it.
    let (release, released) = mpsc::channel::<()>();
    let (held, holding) = mpsc::channel::<()>();
    let _ = window.app().orchestrator_dock.worker.write(move || {
        let _ = held.send(());
        let _ = released.recv();
    });
    holding
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker of the dock holds the stop");
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        let _ = release.send(());
    });

    // The frame that asks for the stop returns at once, and so does a job of the
    // browser worker: the stop waits on a worker of its own.
    let started = Instant::now();
    press_control(&mut window);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the frame of the stop took {:?} while the worker of the dock held it",
        started.elapsed()
    );
    let (done, ran) = mpsc::channel::<()>();
    window.app().browser_create_host.io.write(move || {
        let _ = done.send(());
    });
    let frame_started = Instant::now();
    let _ = window.frame(Vec::new());
    assert!(
        frame_started.elapsed() < Duration::from_secs(2),
        "the frame took {:?} while the stop waited",
        frame_started.elapsed()
    );
    ran.recv_timeout(Duration::from_secs(5))
        .expect("a job of the browser worker runs while the stop waits");

    window.wait_for("the stopped conversation", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Notice { text } if text == STOPPED_NOTICE))
    });
    releasing.join().expect("the thread that releases the worker");
    #[cfg(target_os = "linux")]
    wait_until_no_process("the stop after the held job", child);
    let _ = child;
}

#[test]
fn a_conversation_goes_on_while_another_workspace_is_active() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = Window::with_program(
        WIDE,
        vec![
            editor_workspace_state("chat-a", [40.0, 40.0]),
            editor_workspace_state("chat-b", [1000.0, 40.0]),
        ],
        &program,
    );
    window.focus_workspace("chat-a");
    let _ = window.frame(Vec::new());

    send(&mut window, "a question of the first workspace");
    window.focus_workspace("chat-b");
    let _ = window.frame(Vec::new());

    // The answer of the first workspace arrives while the second one shows.
    window.wait_for("the answer of the hidden workspace", |app| {
        entries_of(app, "chat-a")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    assert!(
        entries_of(window.app(), "chat-b").is_empty(),
        "no message of the second workspace"
    );

    // The active workspace draws nothing of the other conversation.
    let picture = drawn(&mut window);
    assert!(
        !picture
            .iter()
            .any(|drawn| drawn.text == "a question of the first workspace"),
        "the dock draws only the conversation of the active workspace"
    );

    window.focus_workspace("chat-a");
    let picture = drawn(&mut window);
    let person = drawn_text(&picture, "a question of the first workspace");
    let agent = drawn_text(&picture, "the stand-in answers");
    assert!(
        person.rect.min.y < agent.rect.min.y,
        "the conversation of the first workspace shows again: {:?} and {:?}",
        person.rect,
        agent.rect
    );
}

#[test]
fn an_over_long_message_names_the_limit_and_stays_in_the_field() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);
    let text = "x".repeat(horizon_core::agent_bridge::MAX_TEXT_BYTES + 1);
    window.app().orchestrator_dock.draft_mut("chat").push_str(&text);

    // The press of Enter in the field runs this send. The test calls it without a
    // frame, because that frame would lay out a text of 256 KiB.
    let ctx = window.ctx().clone();
    let sent = window.app().say_to_orchestrator(&ctx, "chat", &text);

    assert!(!sent, "a message above the bound does not go out");
    assert_eq!(
        window.app().orchestrator_dock.draft("chat"),
        text,
        "the field keeps the message of the person"
    );
    assert_eq!(
        entries_of(window.app(), "chat"),
        vec![Entry::Notice {
            text: TOO_LONG_NOTICE.to_owned()
        }],
        "one notice names the limit, and no message went out"
    );
    assert_eq!(
        state_of(window.app(), "chat"),
        Some(Phase::NotConnected),
        "no program starts for a message that does not go out"
    );
}

#[test]
fn a_start_that_finds_no_worker_names_the_failure_and_the_next_message_tries_again() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);
    // No thread of the dock takes a job, as a system without the room for one.
    window.app().orchestrator_dock.worker.hold_no_worker();

    send(&mut window, "hello there");
    window.wait_for("the notice of a start without a worker", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Notice { text } if text.starts_with("Claude Code did not start: ")))
    });
    assert_eq!(
        state_of(window.app(), "chat"),
        Some(Phase::Ended),
        "a start that no worker runs does not leave the conversation starting"
    );

    // A worker starts again, and the next message starts a program.
    window.app().orchestrator_dock.worker.take_jobs_again();
    send(&mut window, "and again");
    window.wait_for("the answer of the second start", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    assert_eq!(state_of(window.app(), "chat"), Some(Phase::Running));
}

#[test]
fn the_cost_of_the_turns_stands_left_of_the_chip_of_the_header() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, false, 0.01));
    let mut window = window_of(&program);

    // The first answer, and the second one after the state returned to ready.
    send(&mut window, "one");
    window.wait_for("the first answer", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    send(&mut window, "two");
    window.wait_for("the second answer", |app| {
        entries_of(app, "chat")
            .iter()
            .filter(|entry| matches!(entry, Entry::Agent { .. }))
            .count()
            == 2
    });

    let dock = window
        .app()
        .orchestrator_dock
        .frame_rect()
        .expect("the dock of a wide window");
    let drawn = drawn(&mut window);
    let cost = drawn_text(&drawn, "0.02 USD");
    let chip = drawn_text(&drawn, "ready");
    assert!(
        dock.contains_rect(cost.rect),
        "the cost {:?} stands in the dock {dock:?}",
        cost.rect
    );
    assert!(
        cost.rect.max.x > dock.center().x,
        "the cost {:?} stands at the right edge of the dock {dock:?}",
        cost.rect
    );
    // The chip stands at the right edge with the cost 10 points left of it, on
    // the same centre line, and the second line drops the state word.
    assert!(
        cost.rect.max.x <= chip.rect.min.x - 20.0,
        "the cost {:?} stands left of the chip {:?}",
        cost.rect,
        chip.rect
    );
    assert!((cost.rect.center().y - chip.rect.center().y).abs() <= 0.5);
    assert!(chip.rect.max.x <= dock.max.x - 15.0);
    let line = drawn_text(&drawn, crate::app::orchestrator_dock::header::PROGRAM_LINE);
    assert!(
        line.rect.max.x < cost.rect.min.x,
        "the second line ends before the cost"
    );
}

#[test]
fn the_limit_of_the_plan_is_named_once_for_one_bridge() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let program = stand_in(folder.path(), &answering(&pid, false, true, 0.01));
    let mut window = window_of(&program);

    send(&mut window, "one");
    window.wait_for("the first answer", |app| {
        entries_of(app, "chat")
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    send(&mut window, "two");
    window.wait_for("the second answer", |app| {
        entries_of(app, "chat")
            .iter()
            .filter(|entry| matches!(entry, Entry::Agent { .. }))
            .count()
            == 2
    });

    let notices = entries_of(window.app(), "chat")
        .into_iter()
        .filter(|entry| matches!(entry, Entry::Notice { text } if text == LIMIT_NOTICE))
        .count();
    assert_eq!(notices, 1, "one bridge names the limit one time");
    let picture = drawn(&mut window);
    let notice = drawn_text(&picture, LIMIT_NOTICE);
    assert!(notice.rect.width() > 0.0);
}
