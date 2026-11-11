//! The working directory of the process of a panel, read on the coordination
//! worker.
//!
//! A shell panel follows the directory that its process is in, so a `cd` reaches
//! the session save, the saved board and the sessions that a panel can resume.
//! The read behind that value starts programs on macOS (`pgrep` and `lsof`), so
//! no frame makes it: `Panel::process_output` records the request, this module
//! hands it to the coordination worker, and a later frame keeps the answer.
//!
//! One read runs for each panel at a time, and no panel is read more often than
//! [`INTERVAL`]: a directory changes when the person changes it, not on every
//! line of output. A request waits in the panel until the worker takes it, so a
//! directory that changed while a read was in flight is read again.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use horizon_core::{CwdReader, PROGRAM_BOUND, PanelId, ProgramEnvironment, read_cwd};

use super::HorizonApp;

/// How often one panel is read at most. The session save waits for half a second
/// of quiet in the same way, so a directory that a person just changed reaches
/// the saved board within that time.
const INTERVAL: Duration = Duration::from_millis(500);

/// The reads of the working directories of the panels of this board.
pub(in crate::app) struct CwdReads {
    /// The system that answers on this computer.
    reader: CwdReader,
    /// The programs that the reader runs, and the environment that they see.
    programs: ProgramEnvironment,
    /// The panels whose answer has not come back yet.
    in_flight: HashSet<PanelId>,
    /// When a panel may be read again, so a terminal that prints on every frame
    /// does not send one read per answer.
    next_read: HashMap<PanelId, Instant>,
}

impl Default for CwdReads {
    fn default() -> Self {
        Self::with_programs(CwdReader::of_this_system(), ProgramEnvironment::of_this_process())
    }
}

impl CwdReads {
    /// The reads of a system whose programs are `programs`, which a test points at
    /// stand-in programs of its own folder so the reader of either system runs on
    /// the computer that runs the tests.
    pub(in crate::app) fn with_programs(reader: CwdReader, programs: ProgramEnvironment) -> Self {
        Self {
            reader,
            programs,
            in_flight: HashSet::new(),
            next_read: HashMap::new(),
        }
    }

    /// Records that a read of `panel` runs at `now`, or answers `false` when one
    /// already runs for it or the interval of the last one has not passed. The
    /// request stays in the panel, so the next frame that may read it does.
    fn start(&mut self, panel: PanelId, now: Instant) -> bool {
        if self.in_flight.contains(&panel) || self.next_read.get(&panel).is_some_and(|next| *next > now) {
            return false;
        }
        self.in_flight.insert(panel);
        self.next_read.insert(panel, now + INTERVAL);
        true
    }

    fn finish(&mut self, panel: PanelId) {
        self.in_flight.remove(&panel);
    }

    /// The board that these panels belong to was replaced, and the ids of its
    /// panels name no panel of the new board.
    pub(in crate::app) fn clear(&mut self) {
        self.in_flight.clear();
        self.next_read.clear();
    }

    /// How many panels wait for the answer of their read.
    #[cfg(test)]
    pub(in crate::app) fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }
}

impl HorizonApp {
    /// Asks the coordination worker for the working directory of each panel whose
    /// process may have changed it, once for each panel. The answer of a request
    /// applies on a later frame.
    pub(in crate::app) fn queue_panel_cwd_reads(&mut self) {
        let now = Instant::now();
        for (panel, pid) in self.board.cwd_reads() {
            if !self.cwd_reads.start(panel, now) {
                continue;
            }
            self.board.take_cwd_read(panel);
            let reader = self.cwd_reads.reader;
            let programs = self.cwd_reads.programs.clone();
            self.browser_create_host.io.then(
                move || read_cwd(reader, pid, &programs, PROGRAM_BOUND),
                move |app, answer| app.apply_panel_cwd(panel, pid, answer.flatten()),
            );
        }
    }

    /// Keeps the directory that the worker read, and marks the board for the
    /// session save. An answer without a directory means that no program of this
    /// computer answered within its bound, so the panel keeps the directory that
    /// it had.
    fn apply_panel_cwd(&mut self, panel: PanelId, pid: u32, directory: Option<PathBuf>) {
        self.cwd_reads.finish(panel);
        if let Some(directory) = directory
            && self.board.record_working_directory(panel, pid, directory)
        {
            self.mark_runtime_dirty();
        }
    }

    /// Queues the reads of this frame and applies every answer, for a test that
    /// judges the directory of a panel right after the frame that changed it.
    #[cfg(test)]
    pub(crate) fn settle_panel_cwd_reads(&mut self) {
        self.queue_panel_cwd_reads();
        self.settle_browser_host_io();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use egui::Context;
    use horizon_core::{CwdReader, PanelId, PanelKind, PanelOptions, ProgramEnvironment, stand_in_program_in};

    use super::{CwdReads, INTERVAL};
    use crate::app::test_support::{raw_input, run_app_frame_with_input, test_app};

    /// The folder of stand-in programs of one test, so no program of the person
    /// answers and the reader of macOS runs on the computer that runs the tests.
    struct StandIns {
        folder: tempfile::TempDir,
    }

    impl StandIns {
        fn new() -> Self {
            Self {
                folder: tempfile::tempdir().expect("folder of stand-in programs"),
            }
        }

        fn program(&self, name: &str, body: &str) {
            stand_in_program_in(self.folder.path(), name, body).expect("the stand-in program of the test");
        }

        fn environment(&self) -> ProgramEnvironment {
            ProgramEnvironment::with_path(self.folder.path().as_os_str().to_os_string())
        }

        /// A `pgrep` that names no child, and an `lsof` that answers `directory`.
        fn answer_no_child_and_directory(&self, directory: &std::path::Path) {
            self.program("pgrep", "exit 1");
            self.program("lsof", &format!(r"printf 'p1\nfcwd\nn{0}\n'", directory.display()));
        }
    }

    /// Starts a shell panel that prints once and stays alive, and waits until its
    /// output has reached the terminal of the panel.
    fn panel_with_output(app: &mut crate::app::HorizonApp) -> PanelId {
        let workspace = app.board.create_workspace("directory");
        let panel = app
            .board
            .create_panel_waiting(
                PanelOptions {
                    kind: PanelKind::Shell,
                    command: Some("/bin/sh".to_owned()),
                    args: vec!["-c".to_owned(), "printf 'ready\\n'; sleep 30".to_owned()],
                    ..PanelOptions::default()
                },
                workspace,
            )
            .expect("the panel of the test starts");
        app.settle_panel_starts();
        let deadline = Instant::now() + Duration::from_secs(10);
        while app.board.cwd_reads().is_empty() && Instant::now() < deadline {
            app.board.process_output();
            std::thread::sleep(Duration::from_millis(10));
        }
        panel
    }

    #[test]
    fn a_frame_reads_no_program_for_the_working_directory_of_a_panel() {
        // The strict guard of the UI thread runs in every app of these tests, so
        // a frame that reads the directory itself panics here. The reader of
        // macOS runs with stand-in programs, so this test proves the frame path
        // of the system that starts `pgrep` and `lsof`, on every computer.
        let (temp, mut app) = test_app();
        let stand_ins = StandIns::new();
        let directory = temp.path().join("stand-in-directory");
        std::fs::create_dir_all(&directory).expect("directory of the stand-in");
        stand_ins.answer_no_child_and_directory(&directory);
        app.cwd_reads = CwdReads::with_programs(CwdReader::Programs, stand_ins.environment());
        let ctx = Context::default();
        let panel = panel_with_output(&mut app);
        assert!(
            app.board.cwd_reads().iter().any(|(id, _)| *id == panel),
            "the frame recorded the request of the panel"
        );

        run_app_frame_with_input(&ctx, &mut app, raw_input([1600.0, 1000.0], None));

        assert_eq!(app.cwd_reads.in_flight_count(), 1, "one read runs for the panel");
        assert_eq!(
            app.board.panel(panel).and_then(|panel| panel.launch_cwd.clone()),
            None,
            "the frame kept no directory that it did not read"
        );

        app.settle_panel_cwd_reads();

        assert_eq!(app.cwd_reads.in_flight_count(), 0, "the answer came back");
        assert_eq!(
            app.board.panel(panel).and_then(|panel| panel.launch_cwd.clone()),
            Some(directory),
            "the answer of the stand-in programs reached the panel"
        );
        app.board.shutdown_terminal_panels();
    }

    #[test]
    fn a_panel_is_read_at_most_once_in_the_interval() {
        let mut reads = CwdReads::with_programs(CwdReader::Programs, ProgramEnvironment::with_path("/nonexistent"));
        let panel = PanelId(1);
        let now = Instant::now();

        assert!(reads.start(panel, now), "the first request of a frame is read");
        assert!(!reads.start(panel, now), "a read that runs is not sent again");
        reads.finish(panel);
        assert!(
            !reads.start(panel, Instant::now()),
            "the interval of the last read still holds"
        );
        assert!(
            reads.start(panel, now + INTERVAL + Duration::from_millis(1)),
            "the panel is read again after the interval"
        );
    }

    #[test]
    fn a_second_frame_queues_no_second_read_of_the_same_panel() {
        let (temp, mut app) = test_app();
        let stand_ins = StandIns::new();
        stand_ins.answer_no_child_and_directory(&temp.path().join("stand-in-directory"));
        app.cwd_reads = CwdReads::with_programs(CwdReader::Programs, stand_ins.environment());
        let _panel = panel_with_output(&mut app);

        app.queue_panel_cwd_reads();
        app.queue_panel_cwd_reads();

        assert_eq!(
            app.cwd_reads.in_flight_count(),
            1,
            "a second frame queues no second read of the panel that waits"
        );
        app.settle_panel_cwd_reads();
        assert_eq!(app.cwd_reads.in_flight_count(), 0, "the answer came back");
        app.board.shutdown_terminal_panels();
    }
}
