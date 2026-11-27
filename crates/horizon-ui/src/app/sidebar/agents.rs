//! The group `Agents` of the sidebar: one row for each agent program that the
//! orchestrator of the dock can use, and whether this computer has that program.
//!
//! The group stands under the machine section, in the same scroll area, and only
//! while the orchestrator dock is on. A row does nothing in this part: it says
//! which programs this computer has, and a person reads the group before the dock
//! starts one.
//!
//! The search of the programs reads folders, so it runs on the coordination
//! worker of [`super::super::browser_host_io`], as the search of the presets does.
//! The rows say `looking` until its answer is here, and a frame that draws the
//! group reads that answer without a search. A window whose dock is off draws no
//! group and starts no search.

use egui::{Sense, Stroke, Vec2, WidgetInfo, pos2, vec2};
use horizon_core::{PanelKind, agent_definition};

use crate::app::actions::support::missing_programs;
use crate::theme;

use super::super::HorizonApp;
use super::rows;
use super::text;

/// The title of the group, in the form of the group headers of the sidebar.
pub(in crate::app) const GROUP: &str = "Agents";
/// The second line of a row whose program is on this computer, and of one whose
/// program is not.
pub(in crate::app) const FOUND: &str = "on this computer";
pub(in crate::app) const NOT_FOUND: &str = "not found on this computer";
/// The second line of a row while the search of the programs has no answer yet.
pub(in crate::app) const LOOKING: &str = "looking";
/// The height of one row of the group.
pub(in crate::app) const ROW_HEIGHT: f32 = 34.0;
/// The diameter of the state dot in front of the name of a row.
const DOT_SIDE: f32 = 8.0;
/// The air between the state dot and the two lines of a row.
const DOT_GAP: f32 = 10.0;
/// The air between the two lines of a row.
const LINE_GAP: f32 = 1.0;

/// The four agent programs that the orchestrator can use, in the order of the
/// group. Each row reads its name, its program and its kind from the definition
/// of `horizon_core::agents`, so the group adds no name of its own.
pub(in crate::app) const PROGRAMS: [PanelKind; 4] =
    [PanelKind::Claude, PanelKind::Codex, PanelKind::Pi, PanelKind::Gemini];

/// The number of rows of the group.
const PROGRAM_COUNT: usize = PROGRAMS.len();

/// Whether this computer has the program of one row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum Presence {
    /// The search of the programs has no answer yet.
    #[default]
    Looking,
    Found,
    /// The search found no folder of `PATH` that holds the program.
    Missing,
}

impl Presence {
    /// The second line of the row.
    #[must_use]
    pub(in crate::app) const fn word(self) -> &'static str {
        match self {
            Self::Looking => LOOKING,
            Self::Found => FOUND,
            Self::Missing => NOT_FOUND,
        }
    }

    /// Whether the name of the row takes the full foreground: a program that the
    /// search did not find yet is not a program that this computer lacks.
    const fn lights_the_name(self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// The answer of the search of this run, and whether that search is on the worker.
#[derive(Default)]
pub(in crate::app) struct AgentPrograms {
    /// The presence of each program of [`PROGRAMS`], once the search answered.
    answered: Option<[Presence; PROGRAM_COUNT]>,
    /// Whether the search of this run is on the worker now, so a frame asks once.
    asked: bool,
}

impl AgentPrograms {
    /// The presence of each program of the group, in the order of its rows, and
    /// `looking` for every program until the search answers.
    #[must_use]
    fn presence(&self) -> [Presence; PROGRAM_COUNT] {
        self.answered.unwrap_or([Presence::Looking; PROGRAM_COUNT])
    }

    /// Takes the request of the search of this run: `true` the first time, and
    /// `false` on every frame after it.
    fn ask(&mut self) -> bool {
        if self.asked {
            return false;
        }
        self.asked = true;
        true
    }

    /// Keeps the answer of the search: the indices of the programs of [`PROGRAMS`]
    /// that this computer does not have. A search that did not answer leaves the
    /// rows at `looking`, because Horizon then knows nothing about the programs.
    fn adopt(&mut self, missing: Option<Vec<usize>>) {
        let Some(missing) = missing else {
            return;
        };
        self.answered = Some(std::array::from_fn(|index| {
            if missing.contains(&index) {
                Presence::Missing
            } else {
                Presence::Found
            }
        }));
    }

    /// The rows of a group whose search answered `found`, which a test gives
    /// without a search of the folders of this computer.
    #[cfg(test)]
    pub(in crate::app) fn answered(found: [bool; PROGRAM_COUNT]) -> Self {
        Self {
            answered: Some(std::array::from_fn(|index| {
                if found[index] {
                    Presence::Found
                } else {
                    Presence::Missing
                }
            })),
            asked: true,
        }
    }
}

impl HorizonApp {
    /// Draws the group under the machine section, while the orchestrator dock is
    /// on. A window whose dock is off draws no group, starts no search and builds
    /// no text of the group.
    pub(super) fn render_sidebar_agents(&mut self, ui: &mut egui::Ui) {
        if !self.orchestrator_dock.visible() {
            return;
        }
        self.refresh_agent_programs();
        let presence = self.agent_programs.presence();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(text::line(ui, GROUP, 13.0, text::SEMIBOLD, theme::FG_SOFT(), 120.0));
            ui.add_space(6.0);
            ui.label(text::mono(ui, &PROGRAM_COUNT.to_string(), 11.0, theme::FG_DIM()));
        });
        for (kind, presence) in PROGRAMS.into_iter().zip(presence) {
            render_row(ui, kind, presence);
        }
        ui.add_space(6.0);
    }

    /// Asks the coordination worker which agent programs this computer has, once
    /// for this run. The search reads folders, so no frame runs it.
    fn refresh_agent_programs(&mut self) {
        if !self.agent_programs.ask() {
            return;
        }
        let programs: Vec<(usize, String)> = PROGRAMS
            .iter()
            .enumerate()
            .filter_map(|(index, kind)| {
                agent_definition(*kind).map(|definition| (index, definition.default_command.to_owned()))
            })
            .collect();
        self.browser_create_host.io.then(
            move || missing_programs(&programs),
            |app, missing| app.agent_programs.adopt(missing),
        );
    }
}

/// Draws one row of the group: the state dot, the name of the program and the
/// line under it. The row answers no press, so it carries the shape of the
/// machine rows and no card behind the pointer.
fn render_row(ui: &mut egui::Ui, kind: PanelKind, presence: Presence) {
    let rect = ui.allocate_space(Vec2::new(ui.available_width(), ROW_HEIGHT)).1;
    let card = rect.shrink2(vec2(rows::ROW_INSET, 0.0));
    let dot = pos2(card.min.x + DOT_SIDE / 2.0, card.center().y);
    paint_dot(ui, dot, presence);

    let definition = agent_definition(kind);
    let agent = definition.map_or_else(|| kind.display_name(), |definition| definition.display_name);
    let name_color = if presence.lights_the_name() {
        theme::FG()
    } else {
        theme::FG_DIM()
    };
    let text_left = dot.x + DOT_SIDE / 2.0 + DOT_GAP;
    let text_width = (card.max.x - 8.0 - text_left).max(1.0);
    let name = text::line(ui, agent, 12.5, text::MEDIUM, name_color, text_width);
    let line = text::line(ui, presence.word(), 10.5, text::REGULAR, theme::FG_DIM(), text_width);
    let height = name.size().y + LINE_GAP + line.size().y;
    let top = card.center().y - height / 2.0;
    let name_height = name.size().y;
    let painter = ui.painter();
    painter.galley(pos2(text_left, top), name, name_color);
    painter.galley(pos2(text_left, top + name_height + LINE_GAP), line, theme::FG_DIM());

    // The row informs and takes no press, so a pointer learns nothing there and a
    // screen reader reads the two lines as one label.
    let spoken = format!("{agent}: {}", presence.word());
    let response = ui.interact(
        rect,
        ui.make_persistent_id(("sidebar_agent", kind.display_name())),
        Sense::hover(),
    );
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, &spoken));
}

/// Paints the state dot of a row: filled green for a program of this computer,
/// and hollow while the search runs or when it found the program nowhere.
fn paint_dot(ui: &egui::Ui, center: egui::Pos2, presence: Presence) {
    let radius = DOT_SIDE / 2.0;
    match presence {
        Presence::Found => ui.painter().circle_filled(center, radius, theme::PALETTE_GREEN()),
        Presence::Looking | Presence::Missing => {
            ui.painter()
                .circle_stroke(center, radius - 0.5, Stroke::new(1.0_f32, theme::FG_DIM()))
        }
    };
}

#[cfg(test)]
mod tests {
    use horizon_core::PanelKind;

    use super::{AgentPrograms, PROGRAMS, Presence};

    #[test]
    fn the_rows_of_the_group_follow_the_order_of_the_design() {
        assert_eq!(
            PROGRAMS.map(PanelKind::display_name),
            ["Claude", "Codex", "Pi", "Gemini"]
        );
    }

    #[test]
    fn a_group_without_an_answer_says_looking_on_every_row() {
        let programs = AgentPrograms::default();

        assert_eq!(programs.presence(), [Presence::Looking; PROGRAMS.len()]);
        assert_eq!(Presence::Looking.word(), "looking");
    }

    #[test]
    fn the_answer_of_the_search_marks_the_program_that_this_computer_lacks() {
        let mut programs = AgentPrograms::default();
        assert!(programs.ask(), "the first frame of the group asks once");
        assert!(!programs.ask(), "no frame after it asks again");

        programs.adopt(Some(vec![1, 3]));

        assert_eq!(
            programs.presence(),
            [Presence::Found, Presence::Missing, Presence::Found, Presence::Missing]
        );
        assert_eq!(Presence::Missing.word(), "not found on this computer");
        assert_eq!(Presence::Found.word(), "on this computer");
    }

    /// A search that did not answer says nothing about the programs of this
    /// computer, so the rows keep `looking` and no row claims a missing program.
    #[test]
    fn a_search_that_did_not_answer_leaves_the_rows_at_looking() {
        let mut programs = AgentPrograms::default();
        programs.adopt(None);

        assert_eq!(programs.presence(), [Presence::Looking; PROGRAMS.len()]);
    }
}
