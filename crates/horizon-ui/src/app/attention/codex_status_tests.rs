//! A codex panel, from the program status protocol of its own process to the word
//! of the sidebar.
//!
//! A program reports the state of its turn with OSC 7501, which the VT parser of
//! this workspace drops: the reader of the panel's PTY reads the payload on its way
//! to the parser and hands it to the frame. This test starts the stand-in of such a
//! panel in a real pseudo terminal, types the words of a turn and reads the word
//! that the window shows for the panel. The path is the whole one: the launch of
//! the panel, its real PTY, the reader, the parser of the payload and the turn
//! ledger behind the sidebar.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Attention, Config, PanelId, PanelKind, PanelOptions, RuntimeState, StartupDecision};

use crate::app::HorizonApp;
use crate::app::test_support::{run_app_frame, test_app_with_config_and_startup};

/// The stand-in program of a codex panel of the fixture.
const STAND_IN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/device-smoke/codex_attention_standin.py"
);

/// The longest time that a test waits for a word of the sidebar. It is the hang
/// guard of the test and not a rule of the product: a computer that runs this suite
/// beside other work schedules the program of the panel late.
const PATIENCE: Duration = Duration::from_secs(120);

/// The name of the folder of the panel.
const FOLDER: &str = "work";

fn python3() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|folder| folder.join("python3"))
        .find(|program| program.is_file())
}

/// A codex panel of the stand-in, and the window that shows its word.
struct StandIn {
    /// The state of this test, which holds the folder of the panel.
    _temp: tempfile::TempDir,
    ctx: Context,
    app: HorizonApp,
    id: PanelId,
    local_id: String,
}

impl StandIn {
    /// The window and its one codex panel, or `None` when this computer has no
    /// Python, which the stand-in is written in.
    ///
    /// `protocol` starts a program that reports its state with the program status
    /// protocol, as the real program does; without it the stand-in writes only the
    /// titles of its turn.
    fn start(protocol: bool) -> Option<Self> {
        Self::start_with(if protocol { &["--protocol"] } else { &[] })
    }

    /// The window and its one codex panel whose program starts with `arguments`.
    ///
    /// Every launch of this stand-in writes the start-up phase of the real
    /// program, so a test types its words to a program that is ready for them.
    fn start_with(arguments: &[&str]) -> Option<Self> {
        let python = python3()?;
        let arguments: Vec<&str> = std::iter::once("--startup").chain(arguments.iter().copied()).collect();
        let temp = tempfile::tempdir().expect("isolated state of the test");
        let folder = temp.path().join(FOLDER);
        std::fs::create_dir(&folder).expect("the folder of the panel");
        let (_state, ctx, mut app) = test_app_with_config_and_startup(
            &Config::default(),
            StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            },
        );
        let workspace = app.board.create_workspace("status");
        let args = std::iter::once(STAND_IN.to_owned())
            .chain(arguments.iter().map(|argument| (*argument).to_owned()))
            .collect();
        let id = app
            .board
            .create_panel(
                PanelOptions {
                    name: Some("Codex stand-in".to_owned()),
                    kind: PanelKind::Codex,
                    command: Some(python.display().to_string()),
                    args,
                    cwd: Some(folder),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("a codex panel of the stand-in");
        let local_id = app.board.panel(id).expect("the panel").local_id.clone();
        Some(Self {
            _temp: temp,
            ctx,
            app,
            id,
            local_id,
        })
    }

    /// Types one word into the panel, as the person does.
    fn word(&mut self, word: &str) {
        let panel = self.app.board.panel_mut(self.id).expect("the panel of the test");
        assert!(
            panel.write_input(format!("{word}\n").as_bytes()),
            "the panel of the test takes the word {word}"
        );
    }

    /// Runs the frames of the window until its panel shows `word`.
    fn wait_for(&mut self, word: Attention) -> bool {
        let local_id = self.local_id.clone();
        self.wait_until(move |app| app.panel_attention(&local_id) == word)
    }

    /// Runs the frames of the window until `done` answers, or the patience ends.
    fn wait_until(&mut self, done: impl Fn(&HorizonApp) -> bool) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            run_app_frame(&self.ctx, &mut self.app);
            if done(&self.app) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The word that the sidebar shows for the panel of this test.
    fn shown(&self) -> Attention {
        self.app.panel_attention(&self.local_id)
    }

    /// Runs the frames of the window for `duration`.
    fn run_for(&mut self, duration: Duration, mut each: impl FnMut(&HorizonApp)) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            run_app_frame(&self.ctx, &mut self.app);
            each(&self.app);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn the_reports_of_a_codex_panel_take_the_sidebar_through_its_turn() {
    let Some(mut stand_in) = StandIn::start(true) else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(
        stand_in.wait_for(Attention::Working),
        "the first working report, which showed {:?}",
        stand_in.shown()
    );
    stand_in.word("ask");
    assert!(
        stand_in.wait_for(Attention::NeedsYou),
        "the report of an approval that waits, which showed {:?}",
        stand_in.shown()
    );
    stand_in.word("answer");
    assert!(
        stand_in.wait_for(Attention::Working),
        "the working report after the answer, which showed {:?}",
        stand_in.shown()
    );

    // The person looks at another panel, so the end of the turn is unseen.
    stand_in.app.board.focused = None;
    stand_in.word("stop");
    assert!(
        stand_in.wait_for(Attention::Done),
        "the done report of the turn, which showed {:?}",
        stand_in.shown()
    );
    let local_id = stand_in.local_id.clone();
    assert!(stand_in.app.note_attention_look(&local_id));
    assert_eq!(stand_in.shown(), Attention::Idle);

    // A turn that the person interrupts reports idle and no done.
    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working), "a second turn of the panel");
    stand_in.word("esc");
    assert!(
        stand_in.wait_for(Attention::Done),
        "an interrupted turn is done while unseen, which showed {:?}",
        stand_in.shown()
    );

    // The error state of the protocol is the failed turn of a hook.
    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working));
    stand_in.word("fail");
    assert!(
        stand_in.wait_for(Attention::NeedsYou),
        "a failed turn waits for the person, which showed {:?}",
        stand_in.shown()
    );

    stand_in.word("quit");
    let id = stand_in.id;
    assert!(
        stand_in.wait_until(move |app| app.board.panel(id).is_some_and(|panel| panel.program_end().is_some())),
        "the program of the panel ends after quit"
    );
    let panel = stand_in.app.board.panel(id).expect("the panel of the test");
    assert_eq!(
        stand_in.app.panel_attention_of(panel),
        Attention::Idle,
        "the program that ended claims nothing"
    );
    assert!(
        stand_in.app.attention.states.from_status(&stand_in.local_id),
        "the reports of the launch answered for the panel"
    );
}

#[test]
fn a_report_of_another_program_changes_nothing() {
    let Some(mut stand_in) = StandIn::start(true) else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working));
    let version = stand_in.app.attention.version();
    stand_in.word("foreign");
    let local_id = stand_in.local_id.clone();
    stand_in.run_for(Duration::from_millis(1_500), |app| {
        assert_eq!(
            app.attention.version(),
            version,
            "a report of another program is no state of this panel"
        );
        assert_eq!(app.panel_attention(&local_id), Attention::Working);
    });
    stand_in.word("quit");
}

#[test]
fn a_thousand_repeated_reports_of_one_state_do_no_work() {
    let Some(mut stand_in) = StandIn::start(true) else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working));
    let version = stand_in.app.attention.version();

    stand_in.word("work 1000");
    let local_id = stand_in.local_id.clone();
    stand_in.run_for(Duration::from_millis(1_500), |app| {
        assert_eq!(
            app.attention.version(),
            version,
            "a report of the state that stands is no work"
        );
        assert_eq!(app.panel_attention(&local_id), Attention::Working);
    });
    stand_in.word("quit");
}

#[test]
fn the_reports_of_a_launch_before_its_first_idle_claim_nothing() {
    let Some(mut stand_in) = StandIn::start(true) else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    // The launch of the real program: it reports that it works while it loads,
    // and then reports that it is idle, which is the moment it is ready for the
    // person. No turn of the person is among these reports.
    let version = stand_in.app.attention.version();
    let local_id = stand_in.local_id.clone();
    stand_in.run_for(Duration::from_millis(3_000), |app| {
        assert_eq!(
            app.panel_attention(&local_id),
            Attention::Idle,
            "the reports of a launch claim nothing before its first idle"
        );
    });
    assert_eq!(
        version,
        stand_in.app.attention.version(),
        "the launch of a program changes no row"
    );
}

#[test]
fn a_codex_panel_without_the_protocol_keeps_the_path_of_its_titles() {
    let Some(mut stand_in) = StandIn::start(false) else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(
        stand_in.wait_for(Attention::Working),
        "the spinner of a working turn, which showed {:?}",
        stand_in.shown()
    );
    assert!(
        !stand_in.app.attention.states.from_status(&stand_in.local_id),
        "the launch reported no state, so its reports answer for nothing"
    );
    assert!(
        stand_in.app.attention.states.from_title(&stand_in.local_id),
        "the title of the launch answers for the panel"
    );
    stand_in.word("quit");
}

#[test]
fn a_question_of_a_launch_before_it_is_ready_claims_the_person() {
    let Some(mut stand_in) = StandIn::start_with(&["--protocol", "--startup-question"]) else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    assert!(
        stand_in.wait_for(Attention::NeedsYou),
        "the trust question of a launch, which showed {:?}",
        stand_in.shown()
    );
    // The question stands while the launch blinks it, and the loading reports that
    // follow it claim nothing.
    let local_id = stand_in.local_id.clone();
    stand_in.run_for(Duration::from_millis(1_000), |app| {
        assert_eq!(
            app.panel_attention(&local_id),
            Attention::NeedsYou,
            "the question of a launch stands until the program is ready"
        );
    });
    // The ready report ends the launch. No turn of the person ran in the question,
    // so the row shows no state that waits for them.
    assert!(
        stand_in.wait_for(Attention::Idle),
        "the ready report of a launch clears its own question, which showed {:?}",
        stand_in.shown()
    );
    stand_in.word("quit");
}
