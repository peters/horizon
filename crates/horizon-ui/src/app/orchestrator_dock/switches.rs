//! The switch of the dock and the state that decides whether a frame draws it:
//! the setting of Settings, the environment switch of the fixture, the switch that
//! the palette turned for this run, and the rectangle that the dock took.

use std::collections::HashMap;
use std::path::PathBuf;

use egui::Rect;

use super::front_desk::FrontDesk;
use super::worker;
use super::workers::WorkerBook;
use super::{DOCK_SWITCH, OrchestratorDock, PROGRAM, PROGRAM_SWITCH};

impl Default for OrchestratorDock {
    fn default() -> Self {
        Self::new(false)
    }
}

impl OrchestratorDock {
    /// The dock of a start whose settings hold `setting`: the dock follows that
    /// setting, and the environment switch of the fixture wins over it.
    pub(in crate::app) fn from_settings(setting: bool) -> Self {
        let mut dock = Self::from_values(
            setting,
            dock_switch_value(std::env::var(DOCK_SWITCH).ok().as_deref()),
            program_from_environment(std::env::var(PROGRAM_SWITCH).ok().as_deref()),
        );
        dock.follows_setting = true;
        dock
    }

    /// The dock of a start that names its values: the setting and the environment
    /// switch, which a test gives without the environment of the test process.
    pub(super) fn from_values(setting: bool, environment: Option<bool>, program: PathBuf) -> Self {
        Self {
            setting,
            follows_setting: false,
            environment,
            run_switch: None,
            rect: None,
            program,
            sessions: HashMap::new(),
            drafts: HashMap::new(),
            front_desk: FrontDesk::default(),
            storage: None,
            worker: worker::Worker::default(),
            stops_pending: 0,
            programs: Vec::new(),
            close_wait: None,
            #[cfg(test)]
            last_frame: None,
            #[cfg(test)]
            last_header: None,
            proposal_start: None,
            workers: WorkerBook::default(),
            work_item_states: std::collections::HashMap::new(),
            turn_watch: Vec::new(),
            #[cfg(test)]
            test_running: std::collections::HashSet::new(),
        }
    }

    pub(in crate::app) fn new(visible: bool) -> Self {
        Self::from_values(visible, None, program_from_environment(None))
    }

    /// The dock of a test whose program is `program`: the seam of
    /// [`PROGRAM_SWITCH`], which a test uses without the environment of the test
    /// process.
    #[cfg(test)]
    pub(in crate::app) fn with_program(visible: bool, program: PathBuf) -> Self {
        Self::from_values(visible, None, program)
    }

    /// Replaces the program of every conversation of this dock, which a test does
    /// in place of a whole dock, so the startup decision of the app keeps its
    /// stored folder.
    #[cfg(test)]
    pub(in crate::app) fn set_program(&mut self, program: PathBuf) {
        self.program = program;
    }

    /// Whether the dock draws in this frame. A run that the palette did not turn
    /// keeps the setting of Settings and the environment switch of the fixture,
    /// so a person who changes the setting sees the dock without a restart. The
    /// switch of the run wins over both, and then the switch of the fixture wins
    /// over the setting: `1` shows the dock and `0` hides it.
    pub(in crate::app) fn visible(&self) -> bool {
        self.run_switch
            .unwrap_or_else(|| self.environment.unwrap_or(self.setting))
    }

    pub(in crate::app) fn toggle(&mut self) {
        self.run_switch = Some(!self.visible());
    }

    /// Reads the setting of Settings again. The switch of the palette keeps the
    /// choice of this run, so the dock that a person turned over stays so until
    /// the next start.
    pub(in crate::app) fn follow_setting(&mut self, setting: bool) {
        if self.follows_setting {
            self.setting = setting;
        }
    }

    /// The dock of a test whose switch follows the setting of Settings, as the
    /// dock of a run does, without the environment of the test process.
    #[cfg(test)]
    pub(in crate::app) fn following(setting: bool) -> Self {
        let mut dock = Self::from_values(setting, None, program_from_environment(None));
        dock.follows_setting = true;
        dock
    }

    /// The rectangle of the dock in this frame. A frame that draws no dock has
    /// none, so the canvas keeps the whole window.
    pub(in crate::app) fn frame_rect(&self) -> Option<Rect> {
        self.rect
    }

    pub(in crate::app) fn set_frame_rect(&mut self, rect: Option<Rect>) {
        self.rect = rect;
    }
}

/// Whether the switch of the dock is on: the variable holds `1` and nothing else.
pub(super) fn dock_switch(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Whether the switch of the dock is off: the variable holds `0` and nothing else.
/// The fixture needs a window without the dock in a store that holds the default,
/// which is on.
fn dock_switch_off(value: Option<&str>) -> bool {
    value == Some("0")
}

/// The value of the environment switch of the dock of a run: `Some(true)` for `1`,
/// `Some(false)` for `0`, and `None` when the run names no switch, and then the
/// setting `features.orchestrator_dock` decides.
pub(super) fn dock_switch_value(value: Option<&str>) -> Option<bool> {
    if dock_switch(value) {
        Some(true)
    } else if dock_switch_off(value) {
        Some(false)
    } else {
        None
    }
}

/// The program of a start: the value of [`PROGRAM_SWITCH`] when it names one, and
/// [`PROGRAM`] otherwise.
fn program_from_environment(value: Option<&str>) -> PathBuf {
    value
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(PROGRAM), PathBuf::from)
}
