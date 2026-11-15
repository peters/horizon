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
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use horizon_core::agent_work::{
    TurnLedger, TurnState, WORK_EXECUTABLE_ENV, WORK_KIND_ENV, WORK_OWNER_ENV, WORK_PANEL_ENV, WORK_ROOT_ENV, WorkStore,
};
use horizon_core::{Attention, AttentionBoard, PanelKind};

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

/// A pi panel of the fixture, its record, and the person at its prompt.
struct StandIn {
    /// The run of this test, which holds the home and the report of the panel.
    _root: tempfile::TempDir,
    store: WorkStore,
    child: Child,
    input: ChildStdin,
    answers: BufReader<std::process::ChildStdout>,
}

impl StandIn {
    /// The stand-in of a pi launch, or `None` when this computer has no Python.
    fn start() -> Option<Self> {
        let python = python3()?;
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
            // The launch argument that names the report, which pi loads beside
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
        let answers = BufReader::new(child.stdout.take().expect("stdout of the stand-in"));
        let input = child.stdin.take().expect("stdin of the stand-in");
        Some(Self {
            _root: root,
            store,
            child,
            input,
            answers,
        })
    }

    fn banner(&mut self) -> String {
        let mut line = String::new();
        self.answers.read_line(&mut line).expect("banner of the stand-in");
        line.trim_end().to_string()
    }

    /// Types one word and answers the line that the stand-in printed for it.
    fn word(&mut self, word: &str) -> String {
        writeln!(self.input, "{word}").expect("write the word");
        self.input.flush().expect("flush the word");
        let mut line = String::new();
        self.answers.read_line(&mut line).expect("answer of the stand-in");
        let answer = line.trim_end().to_string();
        assert!(!answer.contains("refused"), "the entry refused {word}: {answer}");
        answer
    }

    /// The record of the panel, once the report of `word` reached it.
    fn wait_for(&self, word: &str, done: impl Fn(&TurnLedger) -> bool) -> TurnLedger {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(record) = self.store.read(PANEL).ok().flatten()
                && done(&record.ledger)
            {
                assert_eq!(record.kind, PanelKind::Pi, "the record is the panel of a pi launch");
                return record.ledger;
            }
            assert!(
                Instant::now() < deadline,
                "the report of {word} did not reach the record of the panel"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The state that the sidebar shows after the record of `word` reached it.
    fn observe(&self, board: &mut AttentionBoard, word: &str, state: TurnState) -> Attention {
        let ledger = self.wait_for(word, |ledger| ledger.state == state);
        assert!(
            !ledger.session_id.is_empty(),
            "the report names the process of its launch after {word}"
        );
        board.observe_ledger(PANEL, &ledger, now_millis());
        assert!(board.from_ledger(PANEL), "the record answers after {word}");
        board.state(PANEL)
    }
}

#[test]
fn the_events_of_a_pi_panel_take_the_sidebar_through_its_turn() {
    let Some(mut stand_in) = StandIn::start() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    let banner = stand_in.banner();
    assert!(
        banner.contains("type one of") || banner.contains("8 events of the report are subscribed"),
        "the stand-in reports the state of its turn: {banner}"
    );
    let mut board = AttentionBoard::default();
    // The launch registered the record and no event of it has arrived: the panel
    // keeps the state of its own screen until the first one does.
    let quiet = stand_in.wait_for("launch", |_| true);
    assert!(quiet.session_id.is_empty(), "pi names no conversation at the launch");
    board.observe_ledger(PANEL, &quiet, now_millis());
    assert!(!board.from_ledger(PANEL), "no event of this launch has arrived");

    for (word, state, shown) in [
        ("start", TurnState::Working, Attention::Working),
        ("ask", TurnState::Blocked, Attention::NeedsYou),
        ("answer", TurnState::Working, Attention::Working),
        ("stop", TurnState::Finished, Attention::Done),
        ("start", TurnState::Working, Attention::Working),
        ("esc", TurnState::Interrupted, Attention::Done),
    ] {
        stand_in.word(word);
        assert_eq!(
            stand_in.observe(&mut board, word, state),
            shown,
            "the sidebar after {word}"
        );
    }
    stand_in.word("quit");
    assert!(stand_in.child.wait().is_ok(), "the stand-in ends after quit");
}

#[test]
fn the_screen_of_a_pi_panel_cannot_end_or_reopen_its_turn() {
    let Some(mut stand_in) = StandIn::start() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.banner();
    let mut board = AttentionBoard::default();
    stand_in.word("start");
    assert_eq!(
        stand_in.observe(&mut board, "start", TurnState::Working),
        Attention::Working
    );
    // The screen of a pi panel repaints a spinner and then holds still, which is
    // the stale timer of the working indicator. That cannot end the turn.
    board.observe_working(PANEL, true, true, now_millis());
    board.observe_working(PANEL, false, false, now_millis());
    assert_eq!(
        board.state(PANEL),
        Attention::Working,
        "a screen without a spinner cannot end a turn that the report holds open"
    );
    stand_in.word("stop");
    assert_eq!(
        stand_in.observe(&mut board, "stop", TurnState::Finished),
        Attention::Done
    );
    // The same rule in the other direction: a line of the screen that looks like
    // a working indicator cannot reopen a turn that the report closed.
    board.observe_working(PANEL, true, false, now_millis());
    assert_eq!(board.state(PANEL), Attention::Done, "an ended turn stays ended");
    // The end of the session is no claim about work, so the record keeps the
    // state of the last turn and shows nothing that waits for the person.
    stand_in.word("quit");
    let ended = stand_in.wait_for("quit", |ledger| ledger.ended_at_millis.is_some());
    board.observe_ledger(PANEL, &ended, now_millis());
    assert_eq!(
        board.state(PANEL),
        Attention::Done,
        "the end of the session claims nothing"
    );
    assert!(stand_in.child.wait().is_ok(), "the stand-in ends after quit");
}
