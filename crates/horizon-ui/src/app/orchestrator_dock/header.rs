//! The header of the dock: the tile and the name of the active workspace, the
//! state chip at its right edge, and the cost of the conversation left of it.
//!
//! The tile is the tile of the workspace row of the sidebar, drawn at the side
//! of the header, so a person reads the same mark in both places.
//!
//! The second line names the program of the dock: `Orchestrator · Claude Code
//! on this computer`. The state of the conversation stands in the chip at the
//! right edge, in the form of the chip of a card, so a person reads the same
//! mark in both places.
//!
//! The cost of the ended turns stands left of the chip while it is above zero.
//! A header that is too narrow for the name, the cost and the chip cuts the name
//! with an ellipsis first, and then hides the cost. The chip stays.

use std::sync::Arc;

use egui::{Galley, Rect, Stroke, Ui, vec2};
use horizon_core::Workspace;

use crate::app::sidebar::rows;
use crate::block_widget::chip::{self, ChipDot};
use crate::theme;
use crate::typeface::Weight;

use super::session::{Phase, Session};
use super::text;

/// The horizontal padding of the header, which the composer keeps too.
const PADDING_X: f32 = 16.0;
const PADDING_Y: f32 = 12.0;
/// The side of the tile and the air between it and the two lines.
const TILE_SIDE: f32 = 26.0;
const TILE_GAP: f32 = 10.0;
const NAME_SIZE: f32 = 13.5;
const STATE_SIZE: f32 = 11.5;
const LINE_GAP: f32 = 2.0;
/// The cost at the right edge of the header, the air between it and the chip,
/// and the air between the text and the chip.
const COST_SIZE: f32 = 11.0;
const COST_GAP: f32 = 10.0;
const TEXT_CHIP_GAP: f32 = 8.0;
/// The least width of the name line before the cost hides. The name is cut with
/// an ellipsis at any width, so the cost goes first.
const NAME_MIN_WIDTH: f32 = 96.0;
/// The second line of a header of the dock: the program, without its state,
/// which the chip names.
pub(super) const PROGRAM_LINE: &str = "Orchestrator \u{b7} Claude Code on this computer";
/// The name line of a window with no active workspace.
pub(super) const NO_WORKSPACE_LINE: &str = "No work item is open";

/// The state chip at the right edge of the header: the words of the state and
/// the mark at its left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Chip {
    /// The words of the chip, which a test reads.
    pub(super) word: &'static str,
    dot: ChipDot,
}

impl Chip {
    /// The chip of `phase`, which has no bridge before its first message.
    pub(super) fn of(phase: Phase, busy: bool) -> Self {
        let dot = match phase {
            Phase::NotConnected | Phase::Stopped => ChipDot::Hollow,
            Phase::Starting => ChipDot::Filled(theme::PALETTE_YELLOW()),
            Phase::Running if busy => ChipDot::Filled(theme::ACCENT()),
            Phase::Running => ChipDot::Filled(theme::PALETTE_GREEN()),
            Phase::Ended => ChipDot::Filled(theme::PALETTE_RED()),
        };
        Self {
            word: phase.word(busy),
            dot,
        }
    }
}

/// What the header names of the conversation of the active workspace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct State {
    /// The chip of the state of the conversation.
    pub(super) chip: Chip,
    /// The cost of the ended turns of the conversation.
    pub(super) cost_usd_total: f64,
}

impl State {
    /// The state of the conversation of `session`, or the state of a workspace
    /// that has no conversation yet.
    pub(super) fn of(session: Option<&Session>) -> Self {
        session.map_or(
            Self {
                chip: Chip::of(Phase::NotConnected, false),
                cost_usd_total: 0.0,
            },
            |session| Self {
                chip: Chip::of(session.phase(), session.busy()),
                cost_usd_total: session.cost_usd_total(),
            },
        )
    }
}

/// The three galleys of the header.
struct Lines {
    name: Arc<Galley>,
    state: Option<Arc<Galley>>,
    cost: Option<Arc<Galley>>,
}

/// The room that the tile of the header takes before its two lines.
fn tile_space(workspace: Option<&Workspace>) -> f32 {
    if workspace.is_some() { TILE_SIDE + TILE_GAP } else { 0.0 }
}

/// The width of the chip of `state`, which stands at the right edge.
fn chip_width(ui: &Ui, state: Option<State>) -> f32 {
    state.map_or(0.0, |state| {
        chip::words_width(ui.painter(), state.chip.word) + TEXT_CHIP_GAP
    })
}

/// The galleys of the header of `width` points: the name of the active
/// `workspace`, the line of the program under it, the cost and the chip at the
/// right.
fn lines(ui: &Ui, width: f32, workspace: Option<&Workspace>, state: Option<State>) -> Lines {
    let column = (width - 2.0 * PADDING_X - tile_space(workspace)).max(0.0);
    let name_column = (column - chip_width(ui, state)).max(0.0);
    let mut cost = state.and_then(|state| cost(ui, state.cost_usd_total, name_column));
    let mut name_width = match &cost {
        Some(cost) => (name_column - cost.size().x - COST_GAP).max(1.0),
        None => name_column,
    };
    if cost.is_some() && name_width < NAME_MIN_WIDTH {
        // The name has less room than it needs beside the cost, so the cost goes
        // and the name keeps the whole column with an ellipsis.
        cost = None;
        name_width = name_column;
    }
    let name = workspace.map_or(NO_WORKSPACE_LINE, |workspace| workspace.name.as_str());
    let name = text::line(ui, name, NAME_SIZE, Weight::Semibold, theme::FG(), name_width);
    // The second line keeps the whole column: the chip and the cost stand on the
    // row of the name, so this line never meets them.
    let state = state.map(|_| text::line(ui, PROGRAM_LINE, STATE_SIZE, Weight::Regular, theme::FG_SOFT(), column));
    Lines { name, state, cost }
}

/// The total of the conversation left of the chip, while it is above zero.
fn cost(ui: &Ui, cost_usd_total: f64, max_width: f32) -> Option<Arc<Galley>> {
    (cost_usd_total > 0.0).then(|| {
        text::mono_line(
            ui,
            &format!("{cost_usd_total:.2} USD"),
            COST_SIZE,
            theme::FG_SOFT(),
            max_width,
        )
    })
}

/// The height of the header of the active `workspace` at `width` points, which
/// the dock works out before it draws the header.
pub(super) fn height(ui: &Ui, width: f32, workspace: Option<&Workspace>, state: Option<State>) -> f32 {
    let lines = lines(ui, width, workspace, state);
    let text_height = lines.state.as_ref().map_or(lines.name.size().y, |state| {
        lines.name.size().y + LINE_GAP + state.size().y
    });
    let chip_height = state.map_or(0.0, |_| chip::height());
    (text_height.max(TILE_SIDE).max(chip_height) + 2.0 * PADDING_Y).ceil()
}

/// Draws the header of the dock for the active `workspace` in `rect`, its line
/// under the name, the cost left of the chip and the chip at the right edge.
pub(super) fn render(ui: &Ui, rect: Rect, workspace: Option<&Workspace>, state: Option<State>) {
    let lines = lines(ui, rect.width(), workspace, state);
    let name_height = lines.name.size().y;
    let text_height = lines
        .state
        .as_ref()
        .map_or(name_height, |state| name_height + LINE_GAP + state.size().y);

    let tile_left = rect.min.x + PADDING_X;
    let text_left = tile_left + tile_space(workspace);
    let text_top = rect.center().y - text_height / 2.0;
    if let Some(workspace) = workspace {
        let tile = Rect::from_min_size(
            egui::pos2(tile_left, rect.center().y - TILE_SIDE / 2.0),
            vec2(TILE_SIDE, TILE_SIDE),
        );
        // The dock shows the active workspace, so its tile is the active one.
        let accent = theme::workspace_accent(workspace.color_idx);
        rows::paint_workspace_tile(ui, tile, &workspace.name, accent, true);
    }

    let painter = ui.painter();
    painter.galley(egui::pos2(text_left, text_top), lines.name, theme::FG());
    let state_top = text_top + name_height + LINE_GAP;
    if let Some(state) = lines.state {
        painter.galley(egui::pos2(text_left, state_top), state, theme::FG_SOFT());
    }

    if let Some(state) = state {
        let width = chip::words_width(painter, state.chip.word);
        // The chip and the cost stand on the row of the name, so the line under
        // them keeps the whole width of the header.
        let chip_center = text_top + name_height / 2.0;
        let chip = Rect::from_min_size(
            egui::pos2(rect.max.x - PADDING_X - width, chip_center - chip::height() / 2.0),
            vec2(width, chip::height()),
        );
        if let Some(cost) = lines.cost {
            // The cost and the chip share one centre line at the right edge.
            let cost_rect = Rect::from_min_size(
                egui::pos2(
                    chip.min.x - COST_GAP - cost.size().x,
                    chip.center().y - cost.size().y / 2.0,
                ),
                cost.size(),
            );
            painter.galley(cost_rect.min, cost, theme::FG_SOFT());
        }
        chip::paint_words(painter, chip, state.chip.word, state.chip.dot);
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
    );
}
