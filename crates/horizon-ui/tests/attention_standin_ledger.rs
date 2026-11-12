//! The attention stand-in of the device fixture against the real hook entry.
//!
//! Horizon gives the panel of an agent the identity of its launch in the
//! environment, and the stand-in sends its events through
//! `<HORIZON_WORK_EXECUTABLE> --agent-work-hook`, which is the entry that the
//! plugin of the work hooks names. This test starts the real script with that
//! environment, types the four words of a turn and reads the ledger of the panel
//! the way the sidebar reads it, so the stand-in and the store must agree on the
//! record, the session and the event shape.
//!
//! A panel reports its state on every Unix system, so this binary runs on Linux
//! and on macOS. It asserts the same four lines on both: the banner of the
//! stand-in, the record of the launch, the state of the ledger after each word,
//! and the state that the sidebar shows for it. A system that is no Unix
//! attaches no ledger, and the test binary has no environment to start on.
#![cfg(unix)]
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use horizon_core::PanelKind;
use horizon_core::agent_work::{
    TurnState, WORK_EXECUTABLE_ENV, WORK_KIND_ENV, WORK_OWNER_ENV, WORK_PANEL_ENV, WORK_ROOT_ENV, WorkStore,
};
use horizon_core::{Attention, AttentionBoard};

const STAND_IN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/agent_attention_standin.py"
);
const PANEL: &str = "attention-stand-in";
const OWNER: &str = "fixture-launch-owner";
const SESSION: &str = "fixture-session";
/// The second shape of the panel entry of the fixture: a Claude panel whose preset
/// has a command of its own, which is this stand-in, and whose work continuation
/// is off. Horizon gives that launch its arguments too, so the sidebar follows the
/// ledger of the panel while no resume is offered.
const PANEL_COMMAND_ENTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/attention_panel_command.json"
));

fn now_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

/// The interpreter that the stand-in needs, when this computer has one.
fn python3() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join("python3"))
        .find(|program| program.is_file())
}

fn read_line(reader: &mut BufReader<std::process::ChildStdout>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).expect("answer of the stand-in");
    line.trim_end().to_string()
}

#[test]
fn the_events_of_a_panel_with_a_command_of_its_own_reach_the_sidebar() {
    let entry: serde_json::Value = serde_json::from_str(PANEL_COMMAND_ENTRY).expect("the entry of the fixture");
    assert_eq!(entry["kind"], serde_json::json!("claude"));
    assert!(
        entry["command"].as_str().is_some_and(|command| !command.is_empty()),
        "the preset of the panel runs a command of its own: {entry}"
    );
    assert_eq!(
        entry["work_resume"]["enabled"],
        serde_json::json!(false),
        "the panel continues nothing after a restart"
    );
    let Some(python) = python3() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    let root = tempfile::tempdir().expect("isolated root");
    let home = root.path().join("home");
    std::fs::create_dir(&home).expect("home directory");
    let horizon_home = root.path().join("horizon-home");
    std::fs::create_dir(&horizon_home).expect("Horizon home");
    let store = WorkStore::new(&horizon_home);
    store
        .register_owner("own-command", PanelKind::Claude, OWNER, Some(SESSION), &home)
        .expect("registered launch");
    let mut child = Command::new(&python)
        .arg(STAND_IN)
        // The launch of either shape names the conversation of the panel.
        .arg("--session-id")
        .arg(SESSION)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("HORIZON_HOME", &horizon_home)
        .env("HORIZON_AGENT_INTEGRATIONS", "off")
        .env(WORK_ROOT_ENV, &horizon_home)
        .env(WORK_PANEL_ENV, "own-command")
        .env(WORK_OWNER_ENV, OWNER)
        .env(WORK_KIND_ENV, "claude")
        .env(WORK_EXECUTABLE_ENV, env!("CARGO_BIN_EXE_horizon"))
        .current_dir(&home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the stand-in");
    let mut answers = BufReader::new(child.stdout.take().expect("stdout of the stand-in"));
    let mut input = child.stdin.take().expect("stdin of the stand-in");
    let banner = read_line(&mut answers);
    assert!(banner.contains("type one of"), "the stand-in asks for a word: {banner}");

    let mut board = AttentionBoard::default();
    // The launch registered its ledger, and no event of it has arrived: the
    // panel keeps the state of its own screen until the first one does.
    let quiet = store
        .read("own-command")
        .expect("read the ledger")
        .expect("the launch registered the record");
    assert!(quiet.ledger.session_id == SESSION);
    board.observe_ledger("own-command", &quiet.ledger, now_millis());
    assert!(!board.from_ledger("own-command"), "no event has arrived");
    board.observe_working("own-command", true, false, now_millis());
    assert_eq!(board.state("own-command"), Attention::Working);

    for (word, expected, shown) in [
        ("start", TurnState::Working, Attention::Working),
        ("stop", TurnState::Finished, Attention::Done),
    ] {
        writeln!(input, "{word}").expect("write the word");
        input.flush().expect("flush the word");
        let answer = read_line(&mut answers);
        assert!(!answer.contains("refused"), "the hook entry refused {word}: {answer}");
        let record = store
            .read("own-command")
            .expect("read the ledger")
            .expect("the launch registered the record");
        assert_eq!(record.ledger.state, expected, "after {word}: {answer}");
        board.observe_ledger("own-command", &record.ledger, now_millis());
        assert!(board.from_ledger("own-command"), "the ledger answers after {word}");
        assert_eq!(board.state("own-command"), shown, "the sidebar after {word}");
    }
    writeln!(input, "quit").expect("write quit");
    input.flush().expect("flush quit");
    drop(input);
    let finished = child.wait().expect("the stand-in ends");
    assert!(finished.success(), "the stand-in ends after quit: {finished:?}");
}

#[test]
fn the_events_of_the_stand_in_reach_the_ledger_that_the_sidebar_reads() {
    let Some(python) = python3() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    let root = tempfile::tempdir().expect("isolated root");
    let home = root.path().join("home");
    std::fs::create_dir(&home).expect("home directory");
    let horizon_home = root.path().join("horizon-home");
    std::fs::create_dir(&horizon_home).expect("Horizon home");
    // A launch of an opted-in panel registers the ledger before its program
    // starts, and the store takes an event only for a registered launch.
    let store = WorkStore::new(&horizon_home);
    store
        .register_owner(PANEL, PanelKind::Claude, OWNER, Some(SESSION), &home)
        .expect("registered launch");
    let mut child = Command::new(&python)
        .arg(STAND_IN)
        // Horizon names the conversation of the launch, and the store accepts an
        // event only for that conversation.
        .arg("--session-id")
        .arg(SESSION)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("HORIZON_HOME", &horizon_home)
        .env("HORIZON_AGENT_INTEGRATIONS", "off")
        .env(WORK_ROOT_ENV, &horizon_home)
        .env(WORK_PANEL_ENV, PANEL)
        .env(WORK_OWNER_ENV, OWNER)
        .env(WORK_KIND_ENV, "claude")
        .env(WORK_EXECUTABLE_ENV, env!("CARGO_BIN_EXE_horizon"))
        .current_dir(&home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the stand-in");
    let mut answers = BufReader::new(child.stdout.take().expect("stdout of the stand-in"));
    let mut input = child.stdin.take().expect("stdin of the stand-in");
    let banner = read_line(&mut answers);
    assert!(banner.contains("type one of"), "the stand-in asks for a word: {banner}");

    let mut board = AttentionBoard::default();
    for (word, expected, shown) in [
        ("start", TurnState::Working, Attention::Working),
        ("ask", TurnState::Blocked, Attention::NeedsYou),
        ("answer", TurnState::Working, Attention::Working),
        ("stop", TurnState::Finished, Attention::Done),
    ] {
        writeln!(input, "{word}").expect("write the word");
        input.flush().expect("flush the word");
        let answer = read_line(&mut answers);
        assert!(!answer.contains("refused"), "the hook entry refused {word}: {answer}");
        let record = store
            .read(PANEL)
            .expect("read the ledger")
            .expect("the launch registered the record");
        assert_eq!(
            record.ledger.session_id, SESSION,
            "the event names the session of the launch"
        );
        assert_eq!(record.ledger.state, expected, "after {word}: {answer}");
        board.observe_ledger(PANEL, &record.ledger, now_millis());
        assert_eq!(board.state(PANEL), shown, "the sidebar after {word}");
    }
    writeln!(input, "quit").expect("write quit");
    input.flush().expect("flush quit");
    drop(input);
    let finished = child.wait().expect("the stand-in ends");
    assert!(finished.success(), "the stand-in ends after quit: {finished:?}");
}
