//! A codex panel, from the title of its own process to the word of the sidebar.
//!
//! Codex writes the state of its turn into the title of the terminal that runs
//! it, and Horizon reads that title through the reader of the panel: no file, no
//! argument and no hook of the program takes part. This test starts the stand-in
//! of such a panel in a real pseudo terminal, types the words of a turn and reads
//! the word that the window shows for the panel.
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

/// The longest time that a test waits for a word of the sidebar, or for the title
/// of the stand-in. It is the hang guard of the test and not a rule of the
/// product: a computer that runs this suite beside other work schedules the
/// program of the panel late.
const PATIENCE: Duration = Duration::from_secs(120);

/// The name of the folder of the panel, which the stand-in writes into its title.
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
    fn start() -> Option<Self> {
        let python = python3()?;
        let temp = tempfile::tempdir().expect("isolated state of the test");
        let folder = temp.path().join(FOLDER);
        std::fs::create_dir(&folder).expect("the folder of the panel");
        let (_state, ctx, mut app) = test_app_with_config_and_startup(
            &Config::default(),
            StartupDecision::Ephemeral {
                runtime_state: Box::new(RuntimeState::default()),
            },
        );
        let workspace = app.board.create_workspace("title");
        let id = app
            .board
            .create_panel(
                PanelOptions {
                    name: Some("Codex stand-in".to_owned()),
                    kind: PanelKind::Codex,
                    command: Some(python.display().to_string()),
                    args: vec![STAND_IN.to_owned()],
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

    /// The title that the panel shows to the person, which this round leaves
    /// unchanged.
    fn title(&self) -> String {
        self.app
            .board
            .panel(self.id)
            .map(|panel| panel.terminal_title.clone())
            .unwrap_or_default()
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
fn the_title_of_a_codex_panel_takes_the_sidebar_through_its_turn() {
    let Some(mut stand_in) = StandIn::start() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(
        stand_in.wait_for(Attention::Working),
        "the spinner of a working turn, which showed {:?} with the title {:?}",
        stand_in.shown(),
        stand_in.title()
    );
    stand_in.word("ask");
    assert!(
        stand_in.wait_for(Attention::NeedsYou),
        "both frames of the approval wait, which showed {:?}",
        stand_in.shown()
    );
    stand_in.word("answer");
    assert!(
        stand_in.wait_for(Attention::Working),
        "the spinner of the same turn after the answer, which showed {:?}",
        stand_in.shown()
    );

    // The person looks at another panel, so the end of the turn is unseen.
    stand_in.app.board.focused = None;
    stand_in.word("stop");
    assert!(
        stand_in.wait_for(Attention::Done),
        "the end of the turn, which showed {:?}",
        stand_in.shown()
    );
    // A look at the panel after the turn ended keeps its row quiet.
    let local_id = stand_in.local_id.clone();
    assert!(stand_in.app.note_attention_look(&local_id));
    assert_eq!(stand_in.shown(), Attention::Idle);

    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working), "a second turn of the panel");
    stand_in.word("esc");
    assert!(
        stand_in.wait_for(Attention::Done),
        "an interrupted turn ends like a finished one, which showed {:?}",
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
    assert_eq!(
        stand_in.app.attention.states.from_title(&stand_in.local_id),
        true,
        "the titles of the launch answered for the panel"
    );
}

#[test]
fn a_plain_title_between_two_parts_of_a_turn_does_not_end_it() {
    let Some(mut stand_in) = StandIn::start() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working));

    // The stand-in writes a plain title for a tenth of a second and then the
    // spinner of the same turn again, and the window runs through the settle time
    // of the plain title and well past it.
    stand_in.word("plain 0.1");
    let local_id = stand_in.local_id.clone();
    stand_in.run_for(Duration::from_millis(1_500), |app| {
        assert_eq!(
            app.panel_attention(&local_id),
            Attention::Working,
            "a plain title between two parts of one turn ends nothing"
        );
    });
    stand_in.word("quit");
}

#[test]
fn a_thousand_spinner_frames_of_one_state_do_no_work() {
    let Some(mut stand_in) = StandIn::start() else {
        eprintln!("skipped: this computer has no python3 on PATH, and the stand-in is a Python program");
        return;
    };
    stand_in.word("start");
    assert!(stand_in.wait_for(Attention::Working));
    let version = stand_in.app.attention.version();

    stand_in.word("spin 1000");
    let local_id = stand_in.local_id.clone();
    stand_in.run_for(Duration::from_millis(1_500), |app| {
        assert_eq!(
            app.attention.version(),
            version,
            "a frame of the state that stands is no work"
        );
        assert_eq!(app.panel_attention(&local_id), Attention::Working);
    });
    stand_in.word("quit");
}

#[test]
fn a_codex_panel_whose_titles_claim_nothing_keeps_the_signal_of_its_screen() {
    let temp = tempfile::tempdir().expect("isolated state of the test");
    let folder = temp.path().join(FOLDER);
    std::fs::create_dir(&folder).expect("the folder of the panel");
    let (_state, ctx, mut app) = test_app_with_config_and_startup(
        &Config::default(),
        StartupDecision::Ephemeral {
            runtime_state: Box::new(RuntimeState::default()),
        },
    );
    let workspace = app.board.create_workspace("title");
    // A program of the same kind whose titles never take one of the forms of the
    // rule: the sidebar keeps the signal of the screen.
    let id = app
        .board
        .create_panel(
            PanelOptions {
                kind: PanelKind::Codex,
                command: Some("/bin/sh".to_owned()),
                args: vec!["-c".to_owned(), "printf '\\033]0;work\\007'; sleep 60".to_owned()],
                cwd: Some(folder),
                ..PanelOptions::default()
            },
            workspace,
        )
        .expect("a codex panel of a plain title");
    let local_id = app.board.panel(id).expect("the panel").local_id.clone();

    let deadline = Instant::now() + PATIENCE;
    while app.board.panel(id).is_some_and(|panel| panel.terminal_title.is_empty()) {
        assert!(Instant::now() < deadline, "the title of the panel never arrived");
        run_app_frame(&ctx, &mut app);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        app.board.panel(id).expect("the panel").terminal_title,
        "work",
        "the title of the screen of the panel"
    );
    assert!(
        !app.attention.states.from_title(&local_id),
        "a title that claims nothing leaves the panel on its screen"
    );
    assert_eq!(app.panel_attention(&local_id), Attention::Idle);
    app.board.shutdown_terminal_panels();
}
