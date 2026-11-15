//! A pi panel, from the report of its launch to the word in the sidebar.
//!
//! Horizon writes the report of a pi panel into its own home and gives the launch
//! `-e <report>` with the five `HORIZON_WORK_*` variables. The report sends the
//! lifecycle events of pi to `<HORIZON_WORK_EXECUTABLE> --agent-work-hook`, which
//! is the real entry of this binary: it keeps the record of the panel, and the
//! sidebar reads the four words from that record.
//!
//! The test starts a stand-in for the program of such a panel. With `node` the
//! stand-in runs the real report, so this test carries the whole path; without
//! `node` it sends the same documents itself, so the record and the sidebar are
//! still checked. Python is the one program that every run needs.
#![cfg(unix)]
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use horizon_core::PanelKind;
use horizon_core::agent_work::{
    WORK_EXECUTABLE_ENV, WORK_KIND_ENV, WORK_OWNER_ENV, WORK_PANEL_ENV, WORK_ROOT_ENV, WorkStore,
};
use horizon_core::{Attention, AttentionBoard};

const STAND_IN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/pi_attention_standin.py"
);
/// The report exactly as the build embeds it, which is the file that an
/// installed Horizon writes into the home of this host.
const REPORT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/plugins/pi/horizon-report.mjs"
));
const PANEL: &str = "pi-stand-in";
const OWNER: &str = "pi-launch-owner";

fn python3() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join("python3"))
        .find(|program| program.is_file())
}

fn now_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn read_line(reader: &mut BufReader<std::process::ChildStdout>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).expect("answer of the stand-in");
    line.trim_end().to_string()
}

/// The ledger of the panel, once the report of `word` reached it.
fn wait_for(
    store: &WorkStore,
    word: &str,
    done: impl Fn(&horizon_core::agent_work::TurnLedger) -> bool,
) -> horizon_core::agent_work::StoredWork {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let record = store.read(PANEL).ok().flatten();
        if let Some(record) = record
            && done(&record.ledger)
        {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "the report of {word} did not reach the record of the panel"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_events_of_a_pi_panel_take_the_sidebar_through_its_turn() {
    let Some(python) = python3() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    let root = tempfile::tempdir().expect("isolated root");
    let home = root.path().join("home");
    std::fs::create_dir(&home).expect("home directory");
    let horizon_home = root.path().join("horizon-home");
    std::fs::create_dir(&horizon_home).expect("Horizon home");
    let report = root.path().join("pi-report.mjs");
    std::fs::write(&report, REPORT).expect("the report file");
    // A pi launch names no conversation, so the launch registers its record
    // without a session id and the report names the process it runs in.
    let store = WorkStore::new(&horizon_home);
    store
        .register_owner(PANEL, PanelKind::Pi, OWNER, None, &home)
        .expect("registered launch");
    let mut child = Command::new(&python)
        .arg(STAND_IN)
        // The launch argument that names the report, and that pi loads beside
        // the extensions of the person.
        .arg("-e")
        .arg(&report)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("HORIZON_HOME", &horizon_home)
        .env("HORIZON_AGENT_INTEGRATIONS", "off")
        .env(WORK_ROOT_ENV, &horizon_home)
        .env(WORK_PANEL_ENV, PANEL)
        .env(WORK_OWNER_ENV, OWNER)
        .env(WORK_KIND_ENV, "pi")
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
    assert!(
        banner.contains("type one of") || banner.contains("8 events of the report are subscribed"),
        "the stand-in reports the state of its turn: {banner}"
    );

    let mut board = AttentionBoard::default();
    // The launch registered the record and no event of it has arrived: the panel
    // keeps the state of its own screen until the first one does.
    let quiet = store.read(PANEL).expect("read the ledger").expect("the record");
    assert!(
        quiet.ledger.session_id.is_empty(),
        "pi names no conversation at the launch"
    );
    board.observe_ledger(PANEL, &quiet.ledger, now_millis());
    assert!(!board.from_ledger(PANEL), "no event of this launch has arrived");

    for (word, expected, shown) in [
        (
            "start",
            horizon_core::agent_work::TurnState::Working,
            Attention::Working,
        ),
        ("ask", horizon_core::agent_work::TurnState::Blocked, Attention::NeedsYou),
        (
            "answer",
            horizon_core::agent_work::TurnState::Working,
            Attention::Working,
        ),
        ("stop", horizon_core::agent_work::TurnState::Finished, Attention::Done),
        (
            "start",
            horizon_core::agent_work::TurnState::Working,
            Attention::Working,
        ),
        ("esc", horizon_core::agent_work::TurnState::Interrupted, Attention::Done),
    ] {
        writeln!(input, "{word}").expect("write the word");
        input.flush().expect("flush the word");
        let answer = read_line(&mut answers);
        assert!(!answer.contains("refused"), "the entry refused {word}: {answer}");
        let record = wait_for(&store, word, |ledger| ledger.state == expected);
        assert_eq!(record.kind, PanelKind::Pi, "the record is the panel of a pi launch");
        assert_eq!(record.ledger.state, expected, "after {word}: {answer}");
        assert!(
            !record.ledger.session_id.is_empty(),
            "the report names the process of its launch after {word}"
        );
        board.observe_ledger(PANEL, &record.ledger, now_millis());
        assert!(board.from_ledger(PANEL), "the record answers after {word}");
        assert_eq!(board.state(PANEL), shown, "the sidebar after {word}");
    }

    // The two checks of an exact source beside the screen rule: the stale timer
    // of a working indicator cannot end a turn that the record holds open, and a
    // working indicator cannot reopen a turn that the record closed.
    board.observe_working(PANEL, false, false, now_millis());
    assert_eq!(board.state(PANEL), Attention::Done, "an ended turn stays ended");
    writeln!(input, "start").expect("write the word");
    input.flush().expect("flush the word");
    let answer = read_line(&mut answers);
    assert!(!answer.contains("refused"), "the entry refused start: {answer}");
    let record = wait_for(&store, "start", |ledger| {
        ledger.state == horizon_core::agent_work::TurnState::Working
    });
    board.observe_ledger(PANEL, &record.ledger, now_millis());
    assert_eq!(board.state(PANEL), Attention::Working);
    // The screen of a pi panel repaints a spinner and then holds still, which is
    // the stale timer of the working indicator. That cannot end the turn.
    board.observe_working(PANEL, true, true, now_millis());
    board.observe_working(PANEL, false, false, now_millis());
    assert_eq!(
        board.state(PANEL),
        Attention::Working,
        "a screen without a spinner cannot end a turn that the report holds open"
    );

    writeln!(input, "quit").expect("write quit");
    input.flush().expect("flush quit");
    let answer = read_line(&mut answers);
    assert!(!answer.contains("refused"), "the entry refused quit: {answer}");
    let record = wait_for(&store, "quit", |ledger| ledger.ended_at_millis.is_some());
    assert_eq!(
        record.ledger.state,
        horizon_core::agent_work::TurnState::Working,
        "the end of the session is no claim about work"
    );
    drop(input);
    let finished = child.wait().expect("the stand-in ends");
    assert!(finished.success(), "the stand-in ends after quit: {finished:?}");
}
