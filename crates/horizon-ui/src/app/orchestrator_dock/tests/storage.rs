//! The stored conversation of a workspace: the entries of an earlier run come
//! back after a restart, the next message resumes its session, a start that
//! saves nothing writes no file, and a workspace that goes takes its file with
//! it.
//!
//! The stand-in of every test is a `/bin/sh` script that the test writes into a
//! folder of its own. It prints hand-written JSON lines of the Claude Code
//! format, reads the lines of the person, and writes its own arguments to a file,
//! so a test reads the `--resume` of the second start. No test starts an agent
//! program.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::agent_bridge::store::{self, INTERRUPTED_RESULT};
use horizon_core::agent_bridge::{Entry, MAX_ENTRIES, ToolState};
use horizon_core::{
    Config, HorizonHome, OrchestratorConversationState, RuntimeState, SessionOpenDisposition, SessionStore,
    StartupDecision,
};

use crate::app::HorizonApp;
use crate::app::orchestrator_dock::session::{Phase, RESUME_FAILURE_NOTICE, Session};
use crate::app::test_support::{app_in, editor_workspace_state, raw_input, run_app_frame_with_input};

/// The window of these tasks: wide enough that the widest dock stands beside the
/// canvas.
const WIDE: [f32; 2] = [1600.0, 1000.0];
/// The workspace of these tasks.
const WORKSPACE: &str = "chat";
/// The session that every stand-in names.
const SESSION: &str = "00000000-0000-4000-8000-000000000001";
/// The session of the record of a saved board, which a start resumes.
const STORED_SESSION: &str = "00000000-0000-4000-8000-00000000dead";
/// The bound of one wait of these tests.
const BOUND: Duration = Duration::from_secs(20);

/// The stand-in of a program that answers each line of the person, names its
/// session and writes its own arguments to `@ARGS@`.
const ANSWERING: &str = r#"printf '%s\n' "$@" > '@ARGS@'
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
  printf '{"type":"assistant","message":{"id":"msg-%s","content":[{"type":"text","text":"the stand-in answers"}]}}\n' "$n"
  printf '%s\n' '{"type":"result","is_error":false,"duration_ms":5}'
done
"#;

/// The stand-in of a program whose first turn opens a tool call that never ends.
const RUNNING_TOOL: &str = r#"printf '%s\n' "$@" > '@ARGS@'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
printf '%s\n' '{"type":"assistant","message":{"id":"msg-1","content":[{"type":"tool_use","id":"toolu-1","name":"Bash","input":{"command":"sleep 600"}}]}}'
while IFS= read -r line; do :; done
"#;

/// The stand-in of a program that ignores the close of its input and the
/// termination signal, so only a stop of the dock ends it.
const STUBBORN: &str = r#"trap '' TERM
echo $$ > '@PID@'
printf '%s\n' '{"type":"system","subtype":"init","session_id":"@SESSION@"}'
while :; do sleep 0.05; done
"#;

/// The stand-in of a program that ends at once, as a program that does not know
/// the session of a `--resume` does.
const ENDS_AT_ONCE: &str = r#"printf '%s\n' "$@" > '@ARGS@'
printf '%s\n' 'no conversation with that session' >&2
exit 1
"#;

/// A window of one app on the home of `folder`, which the tests of the front desk
/// share.
pub(super) struct Harness {
    pub(super) ctx: Context,
    pub(super) app: HorizonApp,
    time: f64,
}

impl Harness {
    /// The app of a start on the home of `folder`, which the startup decision
    /// opens, and whose orchestrator runs `program`.
    pub(super) fn opening(folder: &Path, program: &Path, startup: StartupDecision) -> Self {
        let ctx = Context::default();
        let mut app = app_in(&ctx, folder, &Config::default(), startup);
        app.orchestrator_dock.set_program(program.to_path_buf());
        app.theme_applied = true;
        app.initial_pan_done = true;
        app.startup_workspace_organization_pending = false;
        app.root_viewport_stabilizer = None;
        Self { ctx, app, time: 1.0 }
    }

    /// The app of a first start on the home of `folder`, which saves `state` as a
    /// session of its own.
    pub(super) fn saved(folder: &Path, program: &Path, state: RuntimeState) -> (Self, String) {
        let resolved = store_of(folder)
            .create_session_from_runtime(state)
            .expect("a saved session of the test");
        let session_id = resolved.session_id.clone();
        let harness = Self::opening(
            folder,
            program,
            StartupDecision::Open {
                disposition: SessionOpenDisposition::New,
                session: Box::new(resolved),
            },
        );
        (harness, session_id)
    }

    /// The app of a second start on the home of `folder`, which opens the saved
    /// session `session_id` again.
    pub(super) fn reopened(folder: &Path, program: &Path, session_id: &str) -> Self {
        let resolved = store_of(folder)
            .resume_session(session_id)
            .expect("the saved session loads");
        Self::opening(
            folder,
            program,
            StartupDecision::Open {
                disposition: SessionOpenDisposition::Resume,
                session: Box::new(resolved),
            },
        )
    }

    pub(super) fn frame(&mut self) {
        self.time += 0.016;
        let mut input = raw_input(WIDE, None);
        input.time = Some(self.time);
        let _ = run_app_frame_with_input(&self.ctx, &mut self.app, input);
    }

    /// Runs frames until `wanted` holds, and names what the test waited for when
    /// the bound ends first.
    pub(super) fn wait_for(&mut self, what: &str, wanted: impl Fn(&HorizonApp) -> bool) {
        let deadline = Instant::now() + BOUND;
        while !wanted(&self.app) {
            assert!(Instant::now() < deadline, "the test waited for {what}");
            self.frame();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Sends `text` as the workspace of this test would, and runs the frame that
    /// starts the program.
    fn send(&mut self, text: &str) {
        self.send_to(WORKSPACE, text);
    }

    /// Sends `text` to the conversation of `local_id`, and runs the frame that
    /// starts its program.
    pub(super) fn send_to(&mut self, local_id: &str, text: &str) {
        self.app.send_orchestrator_text(&self.ctx, local_id, text);
        self.frame();
    }

    /// The path of the stored conversation of `local_id`.
    pub(super) fn stored_path_of(&self, local_id: &str) -> PathBuf {
        self.app
            .orchestrator_dock
            .stored_path(local_id)
            .expect("the dock of a saved session stores its conversations")
    }

    /// The path of the stored conversation of the workspace of this test.
    fn stored_path(&self) -> PathBuf {
        self.app
            .orchestrator_dock
            .stored_path(WORKSPACE)
            .expect("the dock of a saved session stores its conversations")
    }

    /// Whether the load of the stored conversation of this workspace waits on the
    /// worker.
    fn is_loading(&self) -> bool {
        self.app
            .orchestrator_dock
            .session(WORKSPACE)
            .is_some_and(Session::is_loading)
    }
}

/// The store of the home of a test, which the apps of that test share.
fn store_of(folder: &Path) -> SessionStore {
    SessionStore::new(
        HorizonHome::from_root(folder.join(".horizon")),
        folder.join("config.json"),
    )
}

/// The board of a test: one workspace with the editor panel that keeps it on
/// the canvas.
pub(super) fn board_state() -> RuntimeState {
    RuntimeState {
        workspaces: vec![editor_workspace_state(WORKSPACE, [40.0, 40.0])],
        ..RuntimeState::default()
    }
}

/// The same board, whose workspace keeps the conversation of an earlier run.
fn board_state_with_record(program: &str) -> RuntimeState {
    let mut state = board_state();
    state.workspaces[0].orchestrator = Some(OrchestratorConversationState {
        session_id: Some(STORED_SESSION.to_owned()),
        program: program.to_owned(),
        allowed_workers: Vec::new(),
    });
    state
}

/// The stand-in program of a test, written into `folder`.
pub(super) fn stand_in(folder: &Path, body: &str, args: &Path) -> PathBuf {
    let body = body
        .replace("@ARGS@", &args.display().to_string())
        .replace("@SESSION@", SESSION);
    horizon_core::stand_in_program_in(folder, "stand-in.sh", &body).expect("the stand-in program of the test")
}

/// The stubborn stand-in of a test, written into `folder`, which writes its own
/// process id to `pid`.
pub(super) fn stubborn_stand_in(folder: &Path, pid: &Path) -> PathBuf {
    let body = STUBBORN
        .replace("@PID@", &pid.display().to_string())
        .replace("@SESSION@", SESSION);
    horizon_core::stand_in_program_in(folder, "stubborn.sh", &body).expect("the stand-in program of the test")
}

/// The process id that the stubborn stand-in of a test wrote into `pid`.
pub(super) fn child_of(pid: &Path) -> u32 {
    let deadline = Instant::now() + BOUND;
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

/// Whether a program of `pid` still runs. Signal 0 reads the process table and
/// sends nothing to the process.
#[cfg(target_os = "linux")]
fn program_runs(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Waits until the program of `pid` ended, and names what the test waited for.
#[cfg(target_os = "linux")]
pub(super) fn wait_until_no_process(what: &str, pid: u32) {
    let deadline = Instant::now() + BOUND;
    while program_runs(pid) {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The name of the file of the stand-in, which the record of a conversation
/// names.
fn stand_in_name(program: &Path) -> String {
    program
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("the stand-in has a name")
        .to_owned()
}

/// The entries of the conversation of the workspace of this test.
fn entries_of(app: &HorizonApp) -> Vec<Entry> {
    app.orchestrator_dock
        .session(WORKSPACE)
        .map(|session| session.snapshot().entries.clone())
        .unwrap_or_default()
}

/// Whether `entries` holds the message of the person and one text of the agent.
fn answered(entries: &[Entry]) -> bool {
    let person = entries
        .iter()
        .any(|entry| matches!(entry, Entry::Person { text, .. } if text == "alpha"));
    let agent = entries.iter().any(|entry| matches!(entry, Entry::Agent { .. }));
    person && agent
}

/// The entries that the file of `path` holds, which a test reads between frames.
pub(super) fn stored_entries(path: &Path) -> Vec<Entry> {
    store::load(path, MAX_ENTRIES)
        .map(|(entries, _)| entries)
        .unwrap_or_default()
}

/// Waits until `wanted` holds for `path`, which the worker changes. It runs no
/// frame.
pub(super) fn wait_for_path(what: &str, path: &Path, wanted: impl Fn(&Path) -> bool) {
    let deadline = Instant::now() + BOUND;
    while !wanted(path) {
        assert!(Instant::now() < deadline, "the test waited for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The arguments that the stand-in of a test wrote, one for each line, and the
/// argument after `flag`.
fn argument_after(path: &Path, flag: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let arguments: Vec<&str> = text.lines().collect();
    arguments
        .windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].to_owned())
}

/// Every conversation file below `folder`, which the test of an unsaved start
/// reads.
fn conversation_files(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "jsonl") {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn a_restart_shows_the_entries_and_the_next_message_resumes_the_session() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let args = folder.path().join("args");
    let program = stand_in(folder.path(), ANSWERING, &args);
    let (mut first, session_id) = Harness::saved(home.path(), &program, board_state());
    let stored = first.stored_path();

    first.frame();
    assert!(
        entries_of(&first.app).is_empty(),
        "a board without a conversation shows nothing"
    );
    first.send("alpha");
    first.wait_for("the answer of the stand-in", |app| answered(&entries_of(app)));
    first.wait_for("the turn in the file of the conversation", |_| {
        answered(&stored_entries(&stored))
    });
    assert!(
        argument_after(&args, "--session-id").is_some(),
        "the first start names a new session"
    );
    assert!(
        argument_after(&args, "--resume").is_none(),
        "the first start resumes nothing"
    );

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        first.app.begin_shutdown();
        first.app.run_exit_cleanup();
    }
    assert!(
        first.app.session_store.flush_runtime_states(BOUND),
        "the board of the session reached the disk"
    );
    drop(first);

    // A second start on the same home and the same session.
    let mut second = Harness::reopened(home.path(), &program, &session_id);
    second.wait_for("the entries of the stored conversation", |app| {
        app.orchestrator_dock
            .session(WORKSPACE)
            .is_some_and(|session| !session.is_loading())
    });
    let restored = entries_of(&second.app);
    assert!(answered(&restored), "the earlier entries came back: {restored:?}");

    second.send("bravo");
    second.wait_for("the start of the second program", |_| {
        argument_after(&args, "--resume").as_deref() == Some(SESSION)
    });
    assert_eq!(
        argument_after(&args, "--resume").as_deref(),
        Some(SESSION),
        "the next message continues the session of the earlier run"
    );
}

#[test]
fn the_frame_returns_while_a_load_of_a_stored_conversation_waits() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let program = stand_in(folder.path(), ANSWERING, &folder.path().join("args"));
    let name = stand_in_name(&program);
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state_with_record(&name));

    // One job of the worker holds it, as a disk that does not answer does, and
    // the load of the stored conversation waits behind it.
    let (release, released) = mpsc::channel::<()>();
    let _ = harness
        .app
        .orchestrator_dock
        .worker
        .then(move || released.recv().is_ok(), |_dock, _released| {});
    let started = Instant::now();
    harness.frame();
    let took = started.elapsed();

    // A loaded machine can take a second for one frame. A frame that waits for
    // the worker never returns, because the job of the test holds it until the
    // release below, so this bound rejects a real wait and no slow machine.
    assert!(took < Duration::from_secs(10), "the frame returned in {took:?}");
    assert!(
        harness.is_loading(),
        "the load waits on the worker and the frame shows the state that it has"
    );
    let _ = release.send(());
    harness.wait_for("the load of the stored conversation", |app| {
        app.orchestrator_dock
            .session(WORKSPACE)
            .is_some_and(|session| !session.is_loading())
    });
}

#[test]
fn the_exit_writes_the_entry_that_a_turn_left_open() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let program = stand_in(folder.path(), RUNNING_TOOL, &folder.path().join("args"));
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state());
    let stored = harness.stored_path();

    harness.frame();
    harness.send("alpha");
    harness.wait_for("the tool call of the stand-in", |app| {
        entries_of(app).iter().any(|entry| {
            matches!(
                entry,
                Entry::Tool {
                    state: ToolState::Running,
                    ..
                }
            )
        })
    });
    harness.wait_for("the message of the person in the file", |_| {
        stored_entries(&stored)
            .iter()
            .any(|entry| matches!(entry, Entry::Person { .. }))
    });
    assert!(
        !stored_entries(&stored)
            .iter()
            .any(|entry| matches!(entry, Entry::Tool { .. })),
        "a tool call that still runs waits for the exit"
    );

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        harness.app.begin_shutdown();
        harness.app.run_exit_cleanup();
    }

    let written = stored_entries(&stored);
    assert!(
        written.iter().any(|entry| matches!(
            entry,
            Entry::Tool { state: ToolState::Failed, result: Some(result), .. }
                if result == INTERRUPTED_RESULT
        )),
        "the exit writes the tool call, which reads as interrupted: {written:?}"
    );
}

#[test]
fn a_start_that_saves_nothing_writes_no_conversation() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let program = stand_in(folder.path(), ANSWERING, &folder.path().join("args"));
    let mut harness = Harness::opening(
        home.path(),
        &program,
        StartupDecision::Ephemeral {
            runtime_state: Box::new(board_state()),
        },
    );

    harness.frame();
    harness.send("alpha");
    harness.wait_for("the answer of the stand-in", |app| answered(&entries_of(app)));

    assert!(
        harness.app.orchestrator_dock.stored_path(WORKSPACE).is_none(),
        "an unsaved start names no conversation folder"
    );
    assert!(
        conversation_files(home.path()).is_empty(),
        "an unsaved start writes no conversation file: {:?}",
        conversation_files(home.path())
    );
    let record = harness
        .app
        .board
        .workspace_id_by_local_id(WORKSPACE)
        .and_then(|id| harness.app.board.workspace(id))
        .and_then(|workspace| workspace.orchestrator.clone());
    assert_eq!(record, None, "an unsaved start writes no record of a session");
}

#[test]
fn a_removed_workspace_removes_its_conversation_file() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let program = stand_in(folder.path(), ANSWERING, &folder.path().join("args"));
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state());
    let stored = harness.stored_path();

    harness.frame();
    harness.send("alpha");
    harness.wait_for("the turn in the file of the conversation", |_| {
        answered(&stored_entries(&stored))
    });
    assert!(stored.exists(), "the conversation of the workspace is stored");

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        // This is the call of the sidebar on the path that removes a workspace.
        harness.app.forget_orchestrator_conversation(WORKSPACE);
    }

    wait_for_path("the delete of the stored conversation", &stored, |path| !path.exists());
}

#[test]
fn a_change_of_the_session_folder_stops_the_programs_of_the_dock() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let pid = folder.path().join("pid");
    let program = stubborn_stand_in(folder.path(), &pid);
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state());

    harness.frame();
    harness.send("a turn that never ends");
    let child = child_of(&pid);
    harness.wait_for("the running state of the conversation", |app| {
        app.orchestrator_dock
            .session(WORKSPACE)
            .is_some_and(|session| session.phase() == Phase::Running)
    });

    // The session folder of the next board takes the conversations of this one,
    // and the programs of those conversations end on the worker of the dock.
    harness.app.orchestrator_dock.set_storage(None);

    assert!(
        harness.app.orchestrator_dock.session(WORKSPACE).is_none(),
        "the conversation of the session that goes is forgotten"
    );
    assert_eq!(harness.app.orchestrator_dock.running_bridges(), 0);
    #[cfg(target_os = "linux")]
    wait_until_no_process("the program of the conversation of the old session", child);
    let _ = child;
}

#[test]
fn a_workspace_that_closes_its_last_panel_removes_its_conversation_file() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let program = stand_in(folder.path(), ANSWERING, &folder.path().join("args"));
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state());
    let stored = harness.stored_path();

    harness.frame();
    harness.send("alpha");
    harness.wait_for("the turn in the file of the conversation", |_| {
        answered(&stored_entries(&stored))
    });
    assert!(stored.exists(), "the conversation of the workspace is stored");
    let panel = harness.app.board.workspaces[0].panels[0];

    {
        let _frame = horizon_core::ui_thread::FrameScope::enter();
        // The close of the last panel of a workspace takes the workspace with it.
        harness.app.close_panel(panel);
    }
    assert!(
        harness.app.board.workspace_id_by_local_id(WORKSPACE).is_none(),
        "the last panel of the workspace closed with it"
    );
    harness.frame();

    assert!(
        harness.app.orchestrator_dock.session(WORKSPACE).is_none(),
        "the conversation of the closed workspace went"
    );
    wait_for_path("the delete of the stored conversation", &stored, |path| !path.exists());
}

#[test]
fn a_session_that_the_program_did_not_continue_is_forgotten() {
    let home = tempfile::tempdir().expect("a home of the test");
    let folder = tempfile::tempdir().expect("a folder of the stand-in");
    let args = folder.path().join("args");
    let program = stand_in(folder.path(), ENDS_AT_ONCE, &args);
    let name = stand_in_name(&program);
    // A saved board whose record names the session of an earlier run.
    let (mut harness, _session_id) = Harness::saved(home.path(), &program, board_state_with_record(&name));

    harness.frame();
    harness.wait_for("the record of the saved conversation", |app| {
        app.board
            .workspace_id_by_local_id(WORKSPACE)
            .and_then(|id| app.board.workspace(id))
            .is_some_and(|workspace| workspace.orchestrator.is_some())
    });
    // The next message resumes the stored session, and the program ends at once.
    harness.send("bravo");
    harness.wait_for("the notice of the session that did not continue", |app| {
        entries_of(app)
            .iter()
            .any(|entry| matches!(entry, Entry::Notice { text } if text == RESUME_FAILURE_NOTICE))
    });
    assert_eq!(
        argument_after(&args, "--resume").as_deref(),
        Some(STORED_SESSION),
        "the program was asked for the stored session"
    );
    harness.frame();
    let record = harness
        .app
        .board
        .workspace_id_by_local_id(WORKSPACE)
        .and_then(|id| harness.app.board.workspace(id))
        .and_then(|workspace| workspace.orchestrator.clone())
        .expect("the record of the conversation stays, so its entries load again");
    assert_eq!(
        record.session_id, None,
        "the session that did not continue is forgotten"
    );

    // The next message starts a new session.
    std::fs::remove_file(&args).expect("the arguments of the first start");
    harness.send("charlie");
    wait_for_path("the start of a new session", &args, |path| {
        argument_after(path, "--session-id").is_some()
    });
}
