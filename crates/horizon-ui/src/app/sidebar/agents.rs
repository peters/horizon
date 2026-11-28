//! The group `Agents` of the sidebar: one chip for each agent program that the
//! orchestrator of the dock can use, and whether this computer has that program.
//!
//! The chips wrap under the caption of their group, two or three to a line, so the
//! group takes one or two lines instead of eight rows. A chip says in its dot and
//! in its hover text what this computer has: a filled dot and the name in the main
//! colour for a program that the search found, a hollow dot and the muted name for
//! one that it did not find or still looks for.
//!
//! The search of the programs reads folders, so it runs on the coordination worker
//! of [`super::super::browser_host_io`], as the search of the presets does. The
//! chips read `looking` until its answer is here, and a frame that draws the group
//! reads that answer without a search. A window whose dock is off draws no group
//! and starts no search.

use egui::{Pos2, Rect, Sense, Stroke, Vec2, WidgetInfo, pos2, vec2};
use horizon_core::{PanelKind, agent_definition};

use crate::app::actions::support::missing_programs;
use crate::theme;

use super::super::HorizonApp;
use super::caption;
use super::text;

/// The second line of a chip whose program is on this computer, and of one whose
/// program is not.
pub(in crate::app) const FOUND: &str = "on this computer";
pub(in crate::app) const NOT_FOUND: &str = "not found on this computer";
/// The word of a chip while the search of the programs has no answer yet.
pub(in crate::app) const LOOKING: &str = "looking";
/// The height of a chip, its radius, its inset at each side of the group, and the
/// gap between two chips and between two lines of them.
pub(in crate::app) const CHIP_HEIGHT: f32 = 24.0;
const CHIP_RADIUS: u8 = 12;
const CHIP_INSET: f32 = 14.0;
const CHIP_GAP: f32 = 6.0;
/// The padding of a chip at its left and at its right.
const PAD_LEFT: f32 = 8.0;
const PAD_RIGHT: f32 = 10.0;
/// The side of the state dot inside a chip and the gap after it.
const DOT_SIDE: f32 = 7.0;
const DOT_GAP: f32 = 6.0;
/// The size of the name of a chip and the stroke of a hollow dot.
const NAME_SIZE: f32 = 11.5;
const DOT_STROKE: f32 = 1.5;

/// The four agent programs that the orchestrator can use, in the order of the
/// group. Each chip reads its name from the definition of `horizon_core::agents`,
/// so the group adds no name of its own.
pub(in crate::app) const PROGRAMS: [PanelKind; 4] =
    [PanelKind::Claude, PanelKind::Codex, PanelKind::Pi, PanelKind::Gemini];

/// The number of chips of the group.
const PROGRAM_COUNT: usize = PROGRAMS.len();

/// Whether this computer has the program of one chip.
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
    /// The word of the hover text of the chip.
    #[must_use]
    pub(in crate::app) const fn word(self) -> &'static str {
        match self {
            Self::Looking => LOOKING,
            Self::Found => FOUND,
            Self::Missing => NOT_FOUND,
        }
    }

    /// Whether the name of the chip takes the full foreground: a program that the
    /// search did not find yet is not a program that this computer lacks.
    const fn lights_the_name(self) -> bool {
        matches!(self, Self::Found)
    }
}

/// The answer of the search of this run, and whether that search is on the worker.
#[derive(Default)]
pub(in crate::app) struct AgentPrograms {
    /// The presence of each program of [`PROGRAMS`], once the search answered.
    answered: Option<[Presence; PROGRAM_COUNT]>,
    /// Whether the search of this run is on the worker now, so a frame asks once.
    asked: bool,
    /// The hover text of each chip in each state that it had, built once: a frame
    /// builds no string of the group.
    spoken: Vec<(usize, Presence, String)>,
}

impl AgentPrograms {
    /// The presence of each program of the group, in the order of its chips, and
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
    /// chips at `looking`, because Horizon then knows nothing about the programs.
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

    /// The hover text and the accessible name of the chip of `index` in `presence`:
    /// `<name>: <word>`. It is built on the frame that the state of the chip
    /// changes and on no other.
    fn spoken(&mut self, index: usize, presence: Presence) -> &str {
        let at = self
            .spoken
            .iter()
            .position(|(held, held_presence, _)| *held == index && *held_presence == presence);
        let at = if let Some(at) = at {
            at
        } else {
            let words = format!("{}: {}", program_name(index), presence.word());
            self.spoken.push((index, presence, words));
            self.spoken.len() - 1
        };
        &self.spoken[at].2
    }

    /// The chips of a group whose search answered `found`, which a test gives
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
            spoken: Vec::new(),
        }
    }
}

/// The name of the program of `index`, as the agent definitions write it.
fn program_name(index: usize) -> &'static str {
    agent_definition(PROGRAMS[index])
        .map_or_else(|| PROGRAMS[index].display_name(), |definition| definition.display_name)
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
        caption::group(ui, caption::AGENTS, None, &[]);
        render_chips(ui, &mut self.agent_programs);
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

/// The place of one chip in the group: its box and the state of its program.
#[derive(Clone, Copy)]
struct Placed {
    rect: Rect,
    presence: Presence,
}

/// Draws the chips of the four programs in the order of [`PROGRAMS`], wrapped
/// into lines of the width of the sidebar.
fn render_chips(ui: &mut egui::Ui, programs: &mut AgentPrograms) {
    let presence = programs.presence();
    let room = (ui.available_width() - 2.0 * CHIP_INSET).max(1.0);
    let names: [std::sync::Arc<egui::Galley>; PROGRAM_COUNT] =
        std::array::from_fn(|index| text::line(ui, program_name(index), NAME_SIZE, text::MEDIUM, theme::FG(), 80.0));
    let widths: [f32; PROGRAM_COUNT] = std::array::from_fn(|index| chip_width(names[index].size().x));

    let mut placed = [Placed {
        rect: Rect::ZERO,
        presence: Presence::Looking,
    }; PROGRAM_COUNT];
    let (mut x, mut line) = (0.0_f32, 0usize);
    for index in 0..PROGRAM_COUNT {
        let width = widths[index];
        if x > 0.0 && x + width > room {
            x = 0.0;
            line += 1;
        }
        let top = usize_to_f32(line) * (CHIP_HEIGHT + CHIP_GAP);
        placed[index] = Placed {
            rect: Rect::from_min_size(pos2(CHIP_INSET + x, top), vec2(width, CHIP_HEIGHT)),
            presence: presence[index],
        };
        x += width + CHIP_GAP;
    }
    let lines = line + 1;
    let height = usize_to_f32(lines) * CHIP_HEIGHT + usize_to_f32(lines - 1) * CHIP_GAP;
    let block = ui.allocate_space(Vec2::new(ui.available_width(), height)).1;

    for (index, chip) in placed.into_iter().enumerate() {
        let rect = chip.rect.translate(block.min.to_vec2());
        paint_chip(ui, rect, &names[index], chip.presence);
        let response = ui.interact(
            rect,
            ui.make_persistent_id(("sidebar_agent_chip", program_name(index))),
            Sense::hover(),
        );
        let spoken = programs.spoken(index, chip.presence);
        response.widget_info(|| WidgetInfo::labeled(egui::Role::Label, true, spoken));
        response.on_hover_text(spoken);
    }
}

/// The width of the chip of a name `name_width` wide: its two paddings, its dot
/// and the gap after the dot.
fn chip_width(name_width: f32) -> f32 {
    PAD_LEFT + DOT_SIDE + DOT_GAP + name_width + PAD_RIGHT
}

/// Paints the chip of `presence` in `rect`: its outline, its dot and its name.
fn paint_chip(ui: &egui::Ui, rect: Rect, name: &std::sync::Arc<egui::Galley>, presence: Presence) {
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CHIP_RADIUS,
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        egui::StrokeKind::Inside,
    );
    let dot = pos2(rect.min.x + PAD_LEFT + DOT_SIDE / 2.0, rect.center().y);
    paint_dot(painter, dot, presence);
    let color = if presence.lights_the_name() {
        theme::FG()
    } else {
        theme::FG_SOFT()
    };
    painter.galley(
        pos2(
            rect.min.x + PAD_LEFT + DOT_SIDE + DOT_GAP,
            rect.center().y - name.size().y / 2.0,
        ),
        name.clone(),
        color,
    );
}

/// Paints the state dot of a chip: filled green for a program of this computer,
/// and hollow while the search runs or when it found the program nowhere.
fn paint_dot(painter: &egui::Painter, center: Pos2, presence: Presence) {
    let radius = DOT_SIDE / 2.0;
    match presence {
        Presence::Found => {
            painter.circle_filled(center, radius, theme::PALETTE_GREEN());
        }
        Presence::Looking | Presence::Missing => {
            painter.circle_stroke(
                center,
                radius - DOT_STROKE / 2.0,
                Stroke::new(DOT_STROKE, theme::FG_DIM()),
            );
        }
    }
}

/// `value` as a float, for the geometry of the group.
fn usize_to_f32(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// The boxes of the chips of a group of `width`, which a test reads: the chip of
/// each program in the order of the rows.
#[cfg(test)]
pub(super) fn chip_rects(width: f32, names: [f32; PROGRAM_COUNT]) -> [Rect; PROGRAM_COUNT] {
    let room = (width - 2.0 * CHIP_INSET).max(1.0);
    let mut placed = [Rect::ZERO; PROGRAM_COUNT];
    let (mut x, mut line) = (0.0_f32, 0usize);
    for (index, name_width) in names.into_iter().enumerate() {
        let chip = chip_width(name_width);
        if x > 0.0 && x + chip > room {
            x = 0.0;
            line += 1;
        }
        let top = usize_to_f32(line) * (CHIP_HEIGHT + CHIP_GAP);
        placed[index] = Rect::from_min_size(pos2(CHIP_INSET + x, top), vec2(chip, CHIP_HEIGHT));
        x += chip + CHIP_GAP;
    }
    placed
}

#[cfg(test)]
mod tests {
    use horizon_core::PanelKind;

    use super::{AgentPrograms, CHIP_GAP, CHIP_HEIGHT, CHIP_INSET, FOUND, LOOKING, NOT_FOUND, PROGRAMS, Presence};
    use super::{chip_rects, chip_width, program_name};

    #[test]
    fn the_chips_of_the_group_follow_the_order_of_the_design() {
        assert_eq!(
            PROGRAMS.map(PanelKind::display_name),
            ["Claude", "Codex", "Pi", "Gemini"]
        );
        assert_eq!(program_name(0), "Claude");
        assert_eq!(program_name(2), "Pi");
    }

    #[test]
    fn a_group_without_an_answer_says_looking_on_every_chip() {
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
    /// computer, so the chips keep `looking` and no chip claims a missing program.
    #[test]
    fn a_search_that_did_not_answer_leaves_the_chips_at_looking() {
        let mut programs = AgentPrograms::default();
        programs.adopt(None);

        assert_eq!(programs.presence(), [Presence::Looking; PROGRAMS.len()]);
    }

    #[test]
    fn the_hover_text_of_a_chip_is_built_once_for_each_state() {
        let mut programs = AgentPrograms::answered([true, false, true, false]);
        let presence = programs.presence();
        assert_eq!(programs.spoken(0, presence[0]), "Claude: on this computer");
        assert_eq!(programs.spoken(1, presence[1]), "Codex: not found on this computer");
        assert_eq!(programs.spoken(0, presence[0]), "Claude: on this computer");
        assert_eq!(
            programs.spoken.len(),
            2,
            "a state that stands still builds no string again"
        );
        assert_eq!(programs.spoken(3, Presence::Looking), "Gemini: looking");
        assert_eq!(programs.spoken.len(), 3);
        assert_eq!(FOUND, "on this computer");
        assert_eq!(NOT_FOUND, "not found on this computer");
        assert_eq!(LOOKING, "looking");
    }

    #[test]
    fn the_chips_wrap_into_lines_inside_the_sidebar() {
        // Four chips of a name of 40 points at the width of the sidebar of the
        // design: three fit on the first line and the fourth goes under them.
        let rects = chip_rects(214.0, [40.0; 4]);
        assert!((rects[0].height() - CHIP_HEIGHT).abs() < f32::EPSILON);
        assert!((rects[0].min.x - CHIP_INSET).abs() < f32::EPSILON);
        assert!((rects[1].min.x - (rects[0].max.x + CHIP_GAP)).abs() < f32::EPSILON);
        assert!((rects[1].min.y - rects[0].min.y).abs() < f32::EPSILON);
        let first_of_the_second_line = rects
            .iter()
            .find(|rect| rect.min.y > rects[0].min.y)
            .expect("a second line of chips");
        assert!((first_of_the_second_line.min.x - CHIP_INSET).abs() < f32::EPSILON);
        assert!((first_of_the_second_line.min.y - rects[0].min.y - CHIP_HEIGHT - CHIP_GAP).abs() < f32::EPSILON);
        assert!(rects.iter().all(|rect| rect.max.x <= 214.0 - CHIP_INSET + f32::EPSILON));
        assert!((chip_width(40.0) - (8.0 + 7.0 + 6.0 + 40.0 + 10.0)).abs() < f32::EPSILON);
    }
}
