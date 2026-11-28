//! The chip `Workers` of the composer of a conversation of a workspace: its value,
//! the menu of the agent programs of the settings, and the press that adds or
//! removes one program.
//!
//! The chip holds the programs that the person lets the orchestrator of this
//! workspace start as workers. A new conversation holds none, so its chip reads
//! `not chosen`; a chip that holds one program names it, and a chip of two or more
//! counts them. The programs are the agent presets of the settings whose program
//! this computer has, which is the question that the group `Agents` of the sidebar
//! answers: no frame searches the folders of the `PATH`, because the search runs on
//! the coordination worker once for this run.
//!
//! The menu is the form of the menu of a list with check marks of the sidebar: one
//! row for each program, with the check mark of a program that the chip holds. A
//! press on a row adds or removes that program and leaves the menu open, so the
//! person chooses two programs with two presses. The escape key and a press outside
//! the menu close it.

use egui::{CursorIcon, Id, Sense, Ui, WidgetInfo};
use horizon_core::agent_definition;

use crate::app::actions::support::missing_programs;
use crate::app::sidebar::menus::checked_row;

use super::OrchestratorDock;
use super::composer::WORKERS_CHIP_ID;

/// The value of a chip that holds no program.
pub(in crate::app) const NONE: &str = "not chosen";
/// The value of a chip that holds `count` programs.
#[must_use]
pub(in crate::app) fn count_value(count: usize) -> String {
    format!("{count} programs")
}
/// The hover text of the chip, which says what a press does.
pub(in crate::app) const CHIP_HOVER: &str = "The programs that this orchestrator may start";

/// One row of the menu of the chip: the name of the preset and whether the chip
/// holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Row {
    pub(in crate::app) program: String,
    pub(in crate::app) held: bool,
}

/// The chip `Workers` of this frame: its value line, its menu rows and the row that
/// the person pressed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct WorkersChip {
    value: String,
    rows: Vec<Row>,
    toggled: Option<usize>,
}

impl WorkersChip {
    /// The value line of the chip, from the programs that it holds.
    pub(in crate::app) fn new(programs: &[String], rows: Vec<Row>) -> Self {
        let value = match programs {
            [] => NONE.to_owned(),
            [only] => only.clone(),
            programs => count_value(programs.len()),
        };
        Self {
            value,
            rows,
            toggled: None,
        }
    }

    /// The value line of the chip.
    pub(in crate::app) fn value(&self) -> &str {
        &self.value
    }

    /// The rows of the menu.
    pub(in crate::app) fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The index of the row that the person pressed this frame, once.
    pub(in crate::app) fn toggled(&self) -> Option<usize> {
        self.toggled
    }

    /// Draws the chip as a control, and the menu of its programs while it is open. A
    /// press on a row records that row, and the frame that reads it changes the chip.
    pub(in crate::app) fn control(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let response = ui.interact(rect, Id::unique(WORKERS_CHIP_ID), Sense::click());
        response.widget_info(|| WidgetInfo::labeled(egui::Role::ComboBox, true, &self.value));
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let response = response.on_hover_text(CHIP_HOVER);
        // A press on a row of the menu leaves the menu open, so the person chooses
        // two programs with two presses. Escape closes the menu, and so does a press
        // outside the menu and outside the chip.
        let popup = egui::Popup::menu(&response).close_behavior(egui::PopupCloseBehavior::IgnoreClicks);
        let mut pressed = None;
        let shown = popup.show(|ui| {
            ui.set_min_width(160.0);
            for (index, row) in self.rows.iter().enumerate() {
                if checked_row(ui, &row.program, row.held, false) {
                    pressed = Some(index);
                }
            }
        });
        if let Some(inner) = &shown
            && pressed_outside(ui, inner.response.rect, rect)
        {
            egui::Popup::close_id(ui.ctx(), egui::Popup::default_response_id(&response));
        }
        if pressed.is_some() {
            self.toggled = pressed;
        }
    }
}

/// Whether this frame holds a press of the pointer outside `menu` and outside the
/// chip at `chip`, which closes the menu of the chip.
fn pressed_outside(ui: &Ui, menu: egui::Rect, chip: egui::Rect) -> bool {
    ui.input(|input| {
        input.pointer.any_pressed()
            && input
                .pointer
                .interact_pos()
                .is_some_and(|pos| !menu.contains(pos) && !chip.contains(pos))
    })
}

impl OrchestratorDock {
    /// Whether the search of the programs of the agent presets of this run is due,
    /// and the request of it is taken.
    pub(in crate::app) fn ask_worker_programs(&mut self) -> bool {
        self.workers.ask_programs()
    }

    /// Keeps the answer of the search of the programs of the agent presets.
    pub(in crate::app) fn adopt_worker_programs(&mut self, missing: Option<Vec<usize>>) {
        self.workers.adopt_programs(missing);
    }

    /// Whether this computer has the program of the agent preset at `index`.
    pub(in crate::app) fn worker_program_found(&self, index: usize) -> bool {
        self.workers.program_found(index)
    }

    /// The chip `Workers` of the conversation of `local_id`, with one row for each
    /// agent preset of `presets` whose program this computer has.
    pub(in crate::app) fn workers_chip(&self, local_id: &str, presets: &[horizon_core::PresetConfig]) -> WorkersChip {
        let held = self.workers_allowed(local_id);
        let rows = presets
            .iter()
            .enumerate()
            .filter(|(index, preset)| agent_preset(preset) && self.worker_program_found(*index))
            .map(|(_, preset)| Row {
                program: preset.name.clone(),
                held: held.iter().any(|program| program == &preset.name),
            })
            .collect();
        WorkersChip::new(held, rows)
    }
}

/// Whether one preset starts an agent program of this computer, so the chip of the
/// workers may hold it: a preset of a browser page, a Device viewer, a card or an
/// SSH terminal runs no agent.
pub(in crate::app) fn agent_preset(preset: &horizon_core::PresetConfig) -> bool {
    preset.kind.is_agent() && preset.requires_workspace_cwd()
}

/// The program that one preset starts: the command that the person saved, or the
/// default command of the kind of its program.
pub(in crate::app) fn preset_program(preset: &horizon_core::PresetConfig) -> Option<String> {
    if let Some(command) = &preset.command {
        return Some(command.clone());
    }
    agent_definition(preset.kind).map(|definition| definition.default_command.to_owned())
}

/// The agent presets of `presets` that start an agent program, with their index.
pub(in crate::app) fn agent_presets(presets: &[horizon_core::PresetConfig]) -> Vec<(usize, String)> {
    presets
        .iter()
        .enumerate()
        .filter(|(_, preset)| agent_preset(preset))
        .filter_map(|(index, preset)| preset_program(preset).map(|program| (index, program)))
        .collect()
}

/// The search of the programs of the agent presets, which reads the folders of the
/// `PATH` and so runs on the coordination worker.
pub(in crate::app) fn search(presets: &[horizon_core::PresetConfig]) -> Vec<usize> {
    missing_programs(&agent_presets(presets))
}

#[cfg(test)]
mod tests {
    use super::{NONE, Row, WorkersChip, agent_preset, count_value, preset_program, search};
    use horizon_core::{PanelKind, PresetConfig};

    fn preset(name: &str, kind: PanelKind) -> PresetConfig {
        PresetConfig {
            name: name.to_owned(),
            alias: None,
            kind,
            command: None,
            args: Vec::new(),
            resume: horizon_core::PanelResume::Fresh,
            ssh_connection: None,
        }
    }

    #[test]
    fn the_value_of_the_chip_names_no_program_one_program_or_a_count() {
        assert_eq!(WorkersChip::new(&[], Vec::new()).value(), NONE);
        assert_eq!(WorkersChip::new(&["Codex".to_owned()], Vec::new()).value(), "Codex");
        assert_eq!(
            WorkersChip::new(&["Codex".to_owned(), "Pi".to_owned()], Vec::new()).value(),
            "2 programs"
        );
        assert_eq!(count_value(3), "3 programs");
        assert_eq!(
            crate::app::sidebar::menus::CHECK_MARK,
            "\u{2713}",
            "the check mark of a menu is Horizon's character"
        );
    }

    #[test]
    fn a_row_of_the_menu_names_the_program_and_whether_the_chip_holds_it() {
        let chip = WorkersChip::new(
            &["Codex".to_owned()],
            vec![
                Row {
                    program: "Codex".to_owned(),
                    held: true,
                },
                Row {
                    program: "Pi".to_owned(),
                    held: false,
                },
            ],
        );
        assert_eq!(chip.rows().len(), 2);
        assert!(chip.rows()[0].held);
        assert_eq!(chip.toggled(), None, "no press, no row");
    }

    #[test]
    fn only_a_preset_of_an_agent_program_is_a_worker_of_the_chip() {
        assert!(agent_preset(&preset("Claude Code", PanelKind::Claude)));
        assert!(!agent_preset(&preset("Task shell", PanelKind::Shell)));
        assert!(!agent_preset(&preset("Page", PanelKind::Browser)));
        assert!(!agent_preset(&preset("Card", PanelKind::Block)));
        assert!(!agent_preset(&preset("Host", PanelKind::Ssh)));
    }

    #[test]
    fn the_program_of_a_preset_is_its_command_or_the_default_of_its_kind() {
        let mut own = preset("Codex", PanelKind::Codex);
        own.command = Some("/home/person/bin/codex-stand-in".to_owned());
        assert_eq!(
            preset_program(&own).as_deref(),
            Some("/home/person/bin/codex-stand-in"),
            "the command that the person saved is the program"
        );
        assert!(
            preset_program(&preset("Pi", PanelKind::Pi)).is_some(),
            "a preset without a command starts the default program of its kind"
        );
        assert_eq!(preset_program(&preset("Card", PanelKind::Block)), None);
    }

    #[test]
    fn the_search_of_the_programs_reads_the_program_of_each_agent_preset() {
        let folder = tempfile::tempdir().expect("a folder");
        let program = horizon_core::stand_in_program_in(folder.path(), "codex-here", "exit 0\n")
            .expect("the program of the test");
        let mut here = preset("Codex", PanelKind::Codex);
        here.command = Some(program.display().to_string());
        let mut gone = preset("Pi", PanelKind::Pi);
        gone.command = Some(folder.path().join("not-there").display().to_string());
        let presets = vec![here, gone, preset("Task shell", PanelKind::Shell)];
        assert_eq!(
            search(&presets),
            vec![1],
            "the preset of a command that is not there is missing, and a shell is no worker"
        );
    }
}
