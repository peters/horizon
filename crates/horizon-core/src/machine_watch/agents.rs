//! The agents of one machine: the panes of its report whose program is one of the
//! agent kinds, with the name, the kind and the two signals of each.
//!
//! A machine names the program of a pane by the name of its foreground process, which
//! is the interpreter for a program that runs through one: `node` for pi, `python3`
//! for a stand-in. The text of the watch therefore reports the command lines of a
//! pane's terminal when an interpreter runs there, and this module finds the kind in
//! the first words of those lines.
//!
//! Every other pane is ignored: a shell, an editor and a plain command are no agent,
//! and a machine cannot make Horizon keep a row by naming a program that no kind of
//! this build uses.

use crate::PanelKind;
use crate::agent_work::TitleState;
use crate::agent_work::title_state;
use crate::agents::{agent_definition, all_agent_kinds};
use crate::config::PresetConfig;
use crate::setup::preset_program;

use crate::is_agent_working_line;

use super::MAX_NAME_CHARS;
use super::report::{PaneReport, Report};

/// The words of a command line of a pane that this module reads, from its first
/// word: an interpreter and the program that it runs.
const COMMAND_WORDS: usize = 3;

/// The programs that name an agent kind of this build, with the kind of each: the
/// default command of every kind of [`crate::agents`] and the program of every preset
/// of the settings whose kind is an agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentPrograms {
    programs: Vec<(String, PanelKind)>,
}

impl AgentPrograms {
    /// The default commands of the agent kinds of this build.
    #[must_use]
    pub fn builtin() -> Self {
        let programs = all_agent_kinds()
            .iter()
            .filter_map(|kind| agent_definition(*kind).map(|definition| (definition.default_command.to_owned(), *kind)))
            .collect();
        Self { programs }
    }

    /// This table with the program of every preset of an agent kind: the file name of
    /// the command of the preset, which is what a `tmux` pane shows ([`preset_program`]).
    #[must_use]
    pub fn with_presets(mut self, presets: &[PresetConfig]) -> Self {
        for preset in presets.iter().filter(|preset| preset.command.is_some()) {
            if agent_definition(preset.kind).is_none() {
                continue;
            }
            let program = preset_program(preset);
            let program = file_name(&program);
            if !program.is_empty() && self.kind_of(program).is_none() {
                self.programs.push((program.to_owned(), preset.kind));
            }
        }
        self
    }

    /// The kind of the agent whose program is `program`, or `None` for another
    /// program.
    #[must_use]
    pub fn kind_of(&self, program: &str) -> Option<PanelKind> {
        self.programs
            .iter()
            .find(|(held, _)| held == program)
            .map(|(_, kind)| *kind)
    }

    /// The kind of the agent of one pane, or `None` for a pane that runs none. The
    /// foreground program answers first; when it is an interpreter, the first words of
    /// the command lines of the pane's terminal answer.
    fn kind_of_pane(&self, pane: &PaneReport) -> Option<PanelKind> {
        if let Some(kind) = self.kind_of(&pane.command) {
            return Some(kind);
        }
        pane.command_lines.iter().find_map(|line| {
            line.split_whitespace()
                .take(COMMAND_WORDS)
                .find_map(|word| self.kind_of(file_name(word)))
        })
    }
}

/// The one thing that a report says about the agent of one pane: the two signals that
/// a state needs, and nothing of the text of the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentObservation {
    /// The pane id of `tmux`.
    pub pane: String,
    /// The `tmux` session of the pane.
    pub session: String,
    /// The agent kind of the pane.
    pub kind: PanelKind,
    /// The name of the agent: the last part of the folder of the pane.
    pub name: String,
    /// The state that the title of the pane claims, or [`TitleState::Plain`].
    pub title: TitleState,
    /// Whether one of the last rows of the pane holds a working line.
    pub working: bool,
}

/// The agents of the panes of `report`, in the order of its panes.
#[must_use]
pub fn observations(report: &Report, programs: &AgentPrograms) -> Vec<AgentObservation> {
    report
        .panes
        .iter()
        .filter_map(|pane| {
            let kind = programs.kind_of_pane(pane)?;
            Some(AgentObservation {
                pane: pane.pane.clone(),
                session: pane.session.clone(),
                kind,
                name: name_of(pane, kind),
                title: title_state(kind, &pane.title),
                working: pane.rows.iter().any(|row| is_agent_working_line(row)),
            })
        })
        .collect()
}

/// The name of the agent of one pane: the last part of its folder, cut at
/// [`MAX_NAME_CHARS`] characters, or the name of its kind for a pane whose folder
/// names no part.
fn name_of(pane: &PaneReport, kind: PanelKind) -> String {
    let folder = pane.cwd.trim_end_matches('/');
    let last = folder.rsplit('/').next().unwrap_or_default();
    let fallback = agent_definition(kind).map_or("", |definition| definition.display_name);
    let name = if last.is_empty() { fallback } else { last };
    name.chars().take(MAX_NAME_CHARS).collect()
}

/// The last part of one word of a command line: the file name of a program or of a
/// script, without its folders.
fn file_name(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}
