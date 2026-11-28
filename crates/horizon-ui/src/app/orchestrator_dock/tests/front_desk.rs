//! The front desk: the one conversation of the dock that belongs to no workspace.
//!
//! These tests read four things: which conversation the dock shows and why, what
//! the frame draws for each of the two forms, the program that a message starts,
//! and the stored side of a conversation that belongs to no workspace.
//!
//! The stand-in of every test is a `/bin/sh` script that the test writes into a
//! folder of its own. It prints hand-written JSON lines of the Claude Code format,
//! reads the lines of the person, writes its own directory and its own process id,
//! and no test starts an agent program.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Event, Modifiers, PointerButton, Pos2, Rect};
use horizon_core::agent_bridge::{Entry, SessionChoice, actor};
use horizon_core::{HorizonHome, RuntimeState, StartupDecision};

use crate::app::SIDEBAR_WIDTH;
use crate::app::orchestrator_dock::composer::{FIELD_HINT, FRONT_DESK_HINT};
use crate::app::orchestrator_dock::empty::{FRONT_DESK_SENTENCES, FRONT_DESK_TITLE};
use crate::app::orchestrator_dock::header::{FRONT_DESK_LINE, FRONT_DESK_TITLE as HEADER_TITLE};
use crate::app::orchestrator_dock::session::Phase;
use crate::app::orchestrator_dock::tests::chat::{self, child_of, entries_of, state_of, stubborn};
use crate::app::orchestrator_dock::tests::frame::{Drawn, Window, texts};
use crate::app::orchestrator_dock::tests::storage::{
    Harness, board_state, child_of as stubborn_child, stand_in, stubborn_stand_in, wait_for_path, wait_until_no_process,
};
use crate::app::orchestrator_dock::{FRONT_DESK, OrchestratorDock};
use crate::app::test_support::{editor_workspace_state, raw_input, run_app_frame_with_input, test_app_with_startup};
use crate::command_registry::CommandId;

/// The window of these tasks: wide enough that the widest dock stands beside the
/// canvas.
pub(super) const WIDE: [f32; 2] = [2000.0, 1000.0];
/// The workspace of these tasks.
const WORKSPACE: &str = "chat";
/// The session that every stand-in names.
const SESSION: &str = "00000000-0000-4000-8000-000000000001";
/// The bound of one wait of these tests.
const BOUND: Duration = Duration::from_secs(20);

/// The stand-in of the front desk: it writes its own directory and its own process
/// id, and it answers every line of the person with one text and one result.
const ANSWERING: &str = r#"printf '%s\n' "$PWD" > '@CWD@'
echo $$ >> '@PIDS@'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '{"type":"assistant","message":{"id":"msg-%s","content":[{"type":"text","text":"the front desk answers"}]}}\n' "$n"
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
done
"#;

/// The two files that the stand-in of these tests writes.
struct Evidence {
    /// The directory that the program of the front desk ran in.
    cwd: PathBuf,
    /// One process id for each start of the program of the front desk.
    pids: PathBuf,
}

/// The evidence of a stand-in inside `folder`.
fn evidence_of(folder: &Path) -> Evidence {
    Evidence {
        cwd: folder.join("cwd"),
        pids: folder.join("pids"),
    }
}

/// The stand-in of the front desk of a test, written into `folder`.
fn front_desk_stand_in(folder: &Path, evidence: &Evidence) -> PathBuf {
    let body = ANSWERING
        .replace("@CWD@", &evidence.cwd.display().to_string())
        .replace("@PIDS@", &evidence.pids.display().to_string())
        .replace("@SESSION@", SESSION);
    horizon_core::stand_in_program_in(folder, "front-desk.sh", &body).expect("the stand-in program of the test")
}

/// The processes that the stand-in of a test started, in their order.
fn started(evidence: &Evidence) -> Vec<u32> {
    std::fs::read_to_string(&evidence.pids)
        .map(|text| text.lines().filter_map(|line| line.trim().parse().ok()).collect())
        .unwrap_or_default()
}

/// The folder of the front desk of this Horizon home.
fn front_desk_directory() -> PathBuf {
    HorizonHome::resolve().root().join(FRONT_DESK)
}

/// A window of one workspace `chat`, whose dock shows the front desk at the start
/// and whose program is `program`.
fn front_desk_window(program: &Path) -> Window {
    Window::with_program(WIDE, vec![editor_workspace_state(WORKSPACE, [40.0, 40.0])], program)
}

/// The entry of the frame that reads `text` at the left of the window, which is
/// the row of the sidebar and not the name in the dock.
fn sidebar_text(drawn: &[Drawn], text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text && drawn.rect.max.x < SIDEBAR_WIDTH)
        .cloned()
        .unwrap_or_else(|| panic!("the sidebar draws {text:?}: {drawn:?}"))
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// Presses the point that `rect` names.
fn press(window: &mut Window, rect: Rect) {
    let at = rect.center();
    let _ = window.frame(vec![Event::PointerMoved(at), button(at, true)]);
    let _ = window.frame(vec![button(at, false)]);
}

/// Clicks the row of the workspace in the sidebar, which shows its conversation,
/// and runs the frame that draws the header of that conversation.
fn press_workspace_row(window: &mut Window) {
    let drawn = texts(&window.frame(Vec::new()));
    let row = sidebar_text(&drawn, WORKSPACE);
    press(window, row.rect);
    let _ = window.frame(Vec::new());
}

/// The key of the conversation that the dock shows in the last frame of `window`.
fn shown(window: &mut Window) -> String {
    let app = window.app();
    app.orchestrator_dock.front_desk().key(&app.board).to_owned()
}

/// Whether the dock of `window` shows the front desk now.
fn shows_front_desk(window: &mut Window) -> bool {
    let app = window.app();
    app.orchestrator_dock.front_desk().shows(&app.board)
}

/// Whether `drawn` holds the words of a chip of the composer.
fn draws_chips(drawn: &[Drawn]) -> bool {
    drawn.iter().any(|drawn| {
        matches!(
            drawn.text.as_str(),
            "Orchestrator" | "Claude Code" | "Workers" | "not chosen" | "Place" | "this computer"
        )
    })
}

/// The first text of `drawn` that reads `text`.
fn drawn_text(drawn: &[Drawn], text: &str) -> Drawn {
    drawn
        .iter()
        .find(|drawn| drawn.text == text)
        .cloned()
        .unwrap_or_else(|| panic!("the frame draws {text:?}: {drawn:?}"))
}

#[test]
fn the_reserved_key_is_no_uuid_and_it_survives_the_workspace_that_goes() {
    assert_eq!(FRONT_DESK, "front-desk");
    assert!(
        !actor::is_uuid(FRONT_DESK),
        "a workspace local id is a UUID, so the reserved key never meets one"
    );

    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);

    chat::send(&mut window, "hello front desk");
    window.wait_for("the answer of the front desk", |app| {
        entries_of(app, FRONT_DESK)
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    press_workspace_row(&mut window);
    assert_eq!(shown(&mut window), WORKSPACE, "the row shows the workspace");

    // The close of the only panel of the workspace takes the workspace and its
    // conversation with it, and the front desk stays.
    let panel = window.app().board.workspaces[0].panels[0];
    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        window.app().close_panel(panel);
    }
    let _ = window.frame(Vec::new());

    assert!(
        window.app().board.workspace_id_by_local_id(WORKSPACE).is_none(),
        "the last panel of the workspace closed with it"
    );
    assert!(
        window.app().orchestrator_dock.session(WORKSPACE).is_none(),
        "the conversation of that workspace went with it"
    );
    assert!(
        window.app().orchestrator_dock.session(FRONT_DESK).is_some(),
        "the front desk belongs to no workspace, so no board takes it away"
    );
}

#[test]
fn the_start_of_horizon_shows_the_front_desk() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    let output = window.frame(Vec::new());

    assert_eq!(shown(&mut window), FRONT_DESK);
    assert!(shows_front_desk(&mut window));
    let drawn = texts(&output);
    let title = drawn_text(&drawn, HEADER_TITLE);
    let line = drawn_text(&drawn, FRONT_DESK_LINE);
    let heading = drawn_text(&drawn, FRONT_DESK_TITLE);
    assert!(
        line.rect.min.y > title.rect.min.y,
        "the job of the front desk stands under its name"
    );
    assert!(heading.rect.min.y > line.rect.max.y);
    assert!(
        drawn
            .iter()
            .any(|drawn| drawn.text.contains("starts a work item for each task")),
        "the empty state says what the front desk does: {drawn:?}"
    );
    assert!(FRONT_DESK_SENTENCES.contains("Write one message with one task or more."));
}

#[test]
fn a_press_on_a_workspace_row_shows_that_workspace() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    let _ = window.frame(Vec::new());
    assert_eq!(shown(&mut window), FRONT_DESK);

    press_workspace_row(&mut window);

    assert_eq!(
        shown(&mut window),
        WORKSPACE,
        "the press shows the workspace of the row"
    );
    let header = window
        .app()
        .orchestrator_dock
        .last_header()
        .expect("the header of the last frame");
    assert!(
        header.back.is_some(),
        "the header of a workspace shows the back control"
    );
    assert!(header.tile.is_some(), "the header of a workspace shows its tile");
}

#[test]
fn a_change_of_the_active_workspace_shows_that_workspace() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = Window::with_program(
        WIDE,
        vec![
            editor_workspace_state(WORKSPACE, [40.0, 40.0]),
            editor_workspace_state("other", [1000.0, 40.0]),
        ],
        &program,
    );
    let _ = window.frame(Vec::new());
    assert_eq!(shown(&mut window), FRONT_DESK, "the start shows the front desk");

    // The board makes the first workspace active, as a press on its row or the
    // palette does. The second one is already active, so only this change is a
    // change of the active workspace.
    let first = window
        .app()
        .board
        .workspace_id_by_local_id(WORKSPACE)
        .expect("the first workspace");
    window.app().board.focus_workspace(first);
    let _ = window.frame(Vec::new());

    assert_eq!(
        shown(&mut window),
        WORKSPACE,
        "the new active workspace shows its conversation"
    );
}

#[test]
fn the_back_control_and_the_palette_command_show_the_front_desk() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    press_workspace_row(&mut window);
    assert_eq!(shown(&mut window), WORKSPACE);

    // The back control, 8 points left of the tile of the header. It is 22 by 22
    // points and stands at the left padding of the header.
    let header = window
        .app()
        .orchestrator_dock
        .last_header()
        .expect("the header of the last frame");
    let back = header.back.expect("the back control of a workspace");
    let tile = header.tile.expect("the tile of a workspace");
    let header_rect = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .header;
    assert_eq!(back.size(), egui::Vec2::new(22.0, 22.0));
    assert_eq!(
        back.min.x - header_rect.min.x,
        16.0,
        "the control stands at the left padding"
    );
    assert_eq!(tile.min.x - back.max.x, 8.0, "the gap between the control and the tile");
    press(&mut window, back);

    assert_eq!(
        shown(&mut window),
        FRONT_DESK,
        "the back control returns to the front desk"
    );

    // The palette command of the front desk.
    press_workspace_row(&mut window);
    assert_eq!(shown(&mut window), WORKSPACE);
    let ctx = window.ctx().clone();
    window.app().execute_command(&ctx, &CommandId::ShowFrontDesk);
    let _ = window.frame(Vec::new());
    assert_eq!(
        shown(&mut window),
        FRONT_DESK,
        "the palette command shows the front desk"
    );
}

#[test]
fn the_sidebar_marks_no_row_while_the_front_desk_shows() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    let _ = window.frame(Vec::new());
    assert_eq!(shown(&mut window), FRONT_DESK);

    let (active, panels) = window.app().sidebar_drawn_rows();
    assert!(active.is_empty(), "no row of the sidebar is the active one: {active:?}");
    assert_eq!(
        panels, 1,
        "a flat list of rows keeps the panel rows of every workspace open"
    );

    // The accordion opens the panel rows of the active workspace alone, so a dock
    // that shows the front desk opens none of them.
    window.app().template_config.features.sidebar_accordion = true;
    let _ = window.frame(Vec::new());
    let (active, panels) = window.app().sidebar_drawn_rows();
    assert!(active.is_empty(), "no row of the sidebar is the active one: {active:?}");
    assert_eq!(panels, 0, "no row of the sidebar opens its panel rows");

    press_workspace_row(&mut window);
    let (active, panels) = window.app().sidebar_drawn_rows();
    assert_eq!(active, vec![WORKSPACE.to_owned()], "the row of the workspace is active");
    assert_eq!(panels, 1, "the workspace opens the row of its panel");

    // The dock that is off keeps the drawing of the sidebar of today.
    window.app().orchestrator_dock.toggle();
    let _ = window.frame(Vec::new());
    assert!(!window.app().orchestrator_dock.visible());
    let (active, _) = window.app().sidebar_drawn_rows();
    assert_eq!(active, vec![WORKSPACE.to_owned()], "the row stays active");
}

#[test]
fn the_sidebar_marks_the_active_row_while_the_dock_is_off() {
    let mut window = Window::new(WIDE, vec![editor_workspace_state(WORKSPACE, [40.0, 40.0])], false);
    window.focus_workspace(WORKSPACE);
    let _ = window.frame(Vec::new());

    assert!(
        !window.app().orchestrator_dock.visible(),
        "the dock of this window is off"
    );
    let (active, panels) = window.app().sidebar_drawn_rows();
    assert_eq!(
        active,
        vec![WORKSPACE.to_owned()],
        "a window whose dock is off marks its active workspace row"
    );
    assert_eq!(panels, 1, "and that row opens its panel rows");
}

#[test]
fn the_header_of_the_front_desk_draws_no_mark_and_the_title_at_the_padding() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    let output = window.frame(Vec::new());

    let header = window
        .app()
        .orchestrator_dock
        .last_header()
        .expect("the header of the last frame");
    assert_eq!(header.back, None, "the front desk has no back control");
    assert_eq!(header.tile, None, "the front desk has no tile");
    let header_rect = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .header;
    assert_eq!(
        header.text_left - header_rect.min.x,
        16.0,
        "the title of the front desk stands at the left padding of the header"
    );
    let title = drawn_text(&texts(&output), HEADER_TITLE);
    assert!((title.rect.min.x - header.text_left).abs() <= 0.5);
}

#[test]
fn the_title_of_a_workspace_follows_its_tile() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    press_workspace_row(&mut window);
    let output = window.frame(Vec::new());

    let header = window
        .app()
        .orchestrator_dock
        .last_header()
        .expect("the header of the last frame");
    let tile = header.tile.expect("the tile of a workspace");
    assert_eq!(header.text_left - tile.max.x, 10.0);
    assert_eq!(
        tile.min.x - header.back.expect("the back control").max.x,
        8.0,
        "the tile follows the back control"
    );
    let name = texts(&output)
        .into_iter()
        .find(|drawn| drawn.text == WORKSPACE && drawn.rect.max.x > SIDEBAR_WIDTH)
        .expect("the name of the workspace in the dock");
    assert!(
        name.rect.min.x > tile.max.x,
        "the title {:?} follows the tile {:?}",
        name.rect,
        tile
    );
    // The tile stands 30 points right of the left padding of the header: 22
    // points of the back control and 8 points of air.
    let header_rect = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock")
        .header;
    assert_eq!(tile.min.x - header_rect.min.x, 46.0);
}

#[test]
fn the_composer_of_the_front_desk_draws_no_chip_and_the_workspace_draws_three() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let program = front_desk_stand_in(folder.path(), &evidence_of(folder.path()));
    let mut window = front_desk_window(&program);
    let output = window.frame(Vec::new());
    let front = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock");

    assert!(
        front.chips.width() <= 0.0 && front.chips.height() <= 0.0,
        "the front desk keeps no room for the chips: {:?}",
        front.chips
    );
    let drawn = texts(&output);
    assert!(!draws_chips(&drawn), "the front desk draws no chip: {drawn:?}");
    assert!(drawn.iter().any(|drawn| drawn.text == FRONT_DESK_HINT));
    assert!(!drawn.iter().any(|drawn| drawn.text == FIELD_HINT));
    // The row of the chips takes no height, so the bottom padding of the composer
    // follows the field.
    assert!(
        (front.composer.max.y - front.field.max.y - 14.0).abs() <= 0.5,
        "the field of the front desk ends 14 points over the bottom edge of its composer: {:?} in {:?}",
        front.field,
        front.composer
    );

    press_workspace_row(&mut window);
    let output = window.frame(Vec::new());
    let workspace = window
        .app()
        .orchestrator_dock
        .last_frame()
        .expect("the frame of the dock");
    let drawn = texts(&output);
    assert!(draws_chips(&drawn), "the workspace draws its three chips: {drawn:?}");
    assert!(drawn.iter().any(|drawn| drawn.text == FIELD_HINT));
    assert!(
        workspace.field.max.y < front.field.max.y,
        "the row of the chips takes the room under the field of a workspace"
    );
}

#[test]
fn a_message_to_the_front_desk_starts_one_program_and_the_next_uses_it() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let evidence = evidence_of(folder.path());
    let program = front_desk_stand_in(folder.path(), &evidence);
    let mut window = front_desk_window(&program);

    chat::send(&mut window, "hello front desk");
    window.wait_for("the answer of the front desk", |app| {
        entries_of(app, FRONT_DESK)
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    window.wait_for("the ready state of the front desk", |app| {
        state_of(app, FRONT_DESK) == Some(Phase::Running)
            && !app
                .orchestrator_dock
                .session(FRONT_DESK)
                .is_some_and(|session| session.snapshot().busy)
    });
    chat::send(&mut window, "and one more task");
    window.wait_for("the second answer of the front desk", |app| {
        entries_of(app, FRONT_DESK)
            .iter()
            .filter(|entry| matches!(entry, Entry::Agent { .. }))
            .count()
            == 2
    });

    assert_eq!(
        started(&evidence).len(),
        1,
        "one program serves both messages: {:?}",
        started(&evidence)
    );
    assert_eq!(
        entries_of(window.app(), FRONT_DESK)
            .iter()
            .filter(|entry| matches!(entry, Entry::Person { .. }))
            .count(),
        2
    );
}

#[test]
fn the_front_desk_runs_in_the_folder_that_the_worker_makes() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let evidence = evidence_of(folder.path());
    let program = front_desk_stand_in(folder.path(), &evidence);
    let mut window = front_desk_window(&program);

    chat::send(&mut window, "hello front desk");
    // The stand-in writes its directory before it answers, and a read that
    // catches the file between the truncation and the write sees no text yet.
    wait_for_path("the directory of the front desk program", &evidence.cwd, |path| {
        std::fs::read_to_string(path).is_ok_and(|text| !text.trim().is_empty())
    });

    let directory = front_desk_directory();
    assert_eq!(
        std::fs::read_to_string(&evidence.cwd)
            .expect("the directory of the stand-in")
            .trim(),
        directory.display().to_string(),
        "the front desk runs in the folder of its own of this Horizon home"
    );
    assert!(
        directory.is_dir(),
        "the folder of the front desk is there before its program runs: {}",
        directory.display()
    );

    // The program of the front desk names the three additions of the design.
    let spec = crate::app::orchestrator_dock::session::program_spec(
        &program,
        &directory,
        SessionChoice::New(SESSION.to_owned()),
        FRONT_DESK,
    );
    let arguments: Vec<String> = spec
        .args
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let value_of = |flag: &str| {
        let at = arguments
            .iter()
            .position(|argument| argument == flag)
            .unwrap_or_else(|| panic!("the front desk names {flag}: {arguments:?}"));
        arguments.get(at + 1).cloned().unwrap_or_default()
    };
    assert_eq!(value_of("--tools"), "", "the front desk has no built-in tool");
    assert_eq!(value_of("--model"), "haiku");
    assert_eq!(
        value_of("--append-system-prompt"),
        horizon_core::agent_bridge::FRONT_DESK_PROMPT
    );
    assert_eq!(spec.cwd, directory);
}

#[test]
fn the_front_desk_conversation_survives_a_restart() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let program = stand_in(folder.path(), ANSWERING_WITH_ARGS, &folder.path().join("args"));
    let (mut first, session_id) = Harness::saved(home.path(), &program, board_state());
    let stored = first.stored_path_of(FRONT_DESK);

    first.frame();
    first.send_to(FRONT_DESK, "alpha");
    first.wait_for("the answer of the front desk", |app| {
        entries_of(app, FRONT_DESK)
            .iter()
            .any(|entry| matches!(entry, Entry::Agent { .. }))
    });
    wait_for_path("the turn in the file of the front desk", &stored, |path| {
        !std::fs::read_to_string(path).unwrap_or_default().is_empty()
    });
    assert!(
        first.app.orchestrator_dock.front_desk().record().is_some(),
        "the record of the front desk stands beside the workspaces of the state"
    );

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        first.app.begin_shutdown();
        first.app.run_exit_cleanup();
    }
    assert!(
        first.app.session_store.flush_runtime_states(BOUND),
        "the state of the session reached the disk"
    );
    drop(first);

    // The state of that session holds the record of the front desk beside its
    // workspaces.
    let path = HorizonHome::from_root(home.path().join(".horizon")).session_runtime_path(&session_id);
    let state = RuntimeState::load(&path)
        .expect("the state of the session loads")
        .expect("the state of the session is there");
    let record = state.front_desk.expect("the state holds the record of the front desk");
    assert_eq!(
        record.program,
        program
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("the name of the stand-in")
    );

    // A second start on the same home and the same session.
    let mut second = Harness::reopened(home.path(), &program, &session_id);
    second.wait_for("the entries of the stored front desk", |app| {
        !entries_of(app, FRONT_DESK).is_empty()
    });
    let restored = entries_of(&second.app, FRONT_DESK);
    assert!(
        restored
            .iter()
            .any(|entry| matches!(entry, Entry::Person { text, .. } if text == "alpha")),
        "the message of the earlier run came back: {restored:?}"
    );
}

/// The stand-in of the front desk that writes its own arguments to `@ARGS@` and
/// answers every line, which the test of a restart reads.
const ANSWERING_WITH_ARGS: &str = r#"printf '%s\n' "$@" > '@ARGS@'
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
  printf '{"type":"assistant","message":{"id":"msg-%s","content":[{"type":"text","text":"the front desk answers"}]}}\n' "$n"
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
done
"#;

#[test]
fn the_close_of_horizon_ends_the_program_of_the_front_desk() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let pid = folder.path().join("pid");
    let body = stubborn(&pid);
    let program = horizon_core::stand_in_program_in(folder.path(), "front-desk.sh", &body)
        .expect("the stand-in program of the test");
    let mut window = front_desk_window(&program);

    chat::send(&mut window, "a turn that never ends");
    let child = child_of(&pid);
    window.wait_for("the running state of the front desk", |app| {
        state_of(app, FRONT_DESK) == Some(Phase::Running)
    });

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        window.app().begin_shutdown();
    }
    let status = chat::exit_status(&mut window);

    assert_eq!(status, 0, "the close of Horizon ends well");
    #[cfg(target_os = "linux")]
    wait_until_no_process("the program of the front desk", child);
    let _ = child;
}

#[test]
fn a_switch_of_the_saved_session_stops_the_program_of_the_front_desk() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let pid = folder.path().join("pid");
    let program = stubborn_stand_in(folder.path(), &pid);
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state());

    harness.frame();
    harness.send_to(FRONT_DESK, "a turn that never ends");
    let child = stubborn_child(&pid);
    harness.wait_for("the running state of the front desk", |app| {
        app.orchestrator_dock
            .session(FRONT_DESK)
            .is_some_and(|session| session.phase() == Phase::Running)
    });

    // The session folder of the next board takes the conversations of this one,
    // and the programs of those conversations end on the worker of the dock.
    harness.app.orchestrator_dock.set_storage(None);

    assert!(
        harness.app.orchestrator_dock.session(FRONT_DESK).is_none(),
        "the front desk of the session that goes is forgotten"
    );
    assert_eq!(harness.app.orchestrator_dock.front_desk().record(), None);
    assert_eq!(harness.app.orchestrator_dock.running_bridges(), 0);
    #[cfg(target_os = "linux")]
    wait_until_no_process("the program of the front desk of the old session", child);
    let _ = child;
}

#[test]
fn the_dock_of_a_board_without_a_workspace_shows_the_front_desk() {
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(RuntimeState::default()),
    });
    app.orchestrator_dock = OrchestratorDock::new(true);
    app.orchestrator_dock.front_desk_mut().show_workspace();
    let mut input = raw_input(WIDE, None);
    input.time = Some(1.0);
    let _ = run_app_frame_with_input(&ctx, &mut app, input);

    assert!(
        app.board.active_workspace.is_none(),
        "the board of this test holds no workspace"
    );
    assert!(
        app.orchestrator_dock.front_desk().shows(&app.board),
        "a board without an active workspace shows the front desk"
    );
    drop(temp);
}
