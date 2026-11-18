//! The close of the main window, and the words of the ask that comes before a
//! close ends programs.
//!
//! The setting `features.when_window_closes` is a choice of three: Horizon asks
//! (the default), keeps running behind a hidden window, or quits. The ask counts
//! what runs once, when the request comes, and speaks from these words. The
//! setting says how this computer runs its programs, so it stays here.

use serde::{Deserialize, Serialize};

use crate::agents::AgentStatus;
use crate::board::Board;

/// The sentence of the agent programs that a restart keeps.
const KEPT_AGENTS: &str = "Agent programs that are kept through a restart come back at the next start.";

/// What a close of the main window does, from the setting
/// `features.when_window_closes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowCloseSetting {
    /// Ask when a program runs, and end at once when none runs.
    #[default]
    Ask,
    /// Hide the window and keep every program of this computer.
    KeepRunning,
    /// End every program and the window.
    Quit,
}

impl WindowCloseSetting {
    /// Every value, in the order of the row of the settings.
    pub const VALUES: [Self; 3] = [Self::Ask, Self::KeepRunning, Self::Quit];

    /// The value as the row of the settings shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::KeepRunning => "Keep Horizon running",
            Self::Quit => "Quit Horizon",
        }
    }
}

/// What runs inside this Horizon when a close request comes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunningPrograms {
    /// Terminal panels of this computer whose program has not ended.
    pub here: usize,
    /// Agent programs of `here` that are in the working state.
    pub working_agents: usize,
    /// Agent programs of `here` that a holder keeps across a restart.
    pub kept_agents: usize,
    /// Panels that run on a machine and keep running there.
    pub on_machines: usize,
}

impl RunningPrograms {
    /// Whether nothing runs that a close would end or leave behind.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.here == 0 && self.on_machines == 0
    }

    /// The line that counts the programs of this computer, with the agents that
    /// work as its second sentence. `None` when no program of this computer runs.
    #[must_use]
    pub fn programs_line(&self) -> Option<String> {
        let programs = match self.here {
            0 => return None,
            1 => "1 program is running.".to_owned(),
            here => format!("{here} programs are running."),
        };
        Some(match self.working_agents {
            0 => programs,
            1 => format!("{programs} 1 agent is working."),
            working => format!("{programs} {working} agents are working."),
        })
    }

    /// The lines of what outlives the window: the panels that a machine keeps, and
    /// for a close that ends this Horizon (`ending`) the agent programs that come
    /// back at the next start.
    #[must_use]
    pub fn outliving_lines(&self, ending: bool) -> Vec<String> {
        let mut lines = match self.on_machines {
            0 => Vec::new(),
            1 => vec!["1 panel runs on a machine and keeps running there.".to_owned()],
            on_machines => vec![format!("{on_machines} panels run on a machine and keep running there.")],
        };
        if ending && self.kept_agents > 0 {
            lines.push(KEPT_AGENTS.to_owned());
        }
        lines
    }

    /// The question of `Quit Horizon` in the menu.
    #[must_use]
    pub fn quit_question(&self) -> String {
        match self.here {
            0 => "Quit Horizon?".to_owned(),
            1 => "End 1 program and quit?".to_owned(),
            here => format!("End {here} programs and quit?"),
        }
    }
}

impl Board {
    /// What runs now: the terminal panels of this computer whose program has not
    /// ended, the agents among them, and the panels that run on a machine.
    #[must_use]
    pub fn running_programs(&self, keeps_agents: bool) -> RunningPrograms {
        let mut running = RunningPrograms::default();
        for panel in &self.panels {
            if panel.machine().is_some() {
                running.on_machines += 1;
            } else if panel.runs_a_program_here() {
                running.here += 1;
                if panel.agent_status() == AgentStatus::Working {
                    running.working_agents += 1;
                }
                if keeps_agents && panel.kind.is_agent() {
                    running.kept_agents += 1;
                }
            }
        }
        running
    }
}

#[cfg(test)]
mod tests;
