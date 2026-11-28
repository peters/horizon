//! The header of the dock: the mark and the name of the conversation that the
//! dock shows, the state chip at its right edge, and the cost of the conversation
//! left of it.
//!
//! The dock shows one of two conversations, so the header has two forms. The
//! front desk belongs to no workspace: its name stands at the left padding and no
//! mark stands before it. A workspace shows the tile of its row of the sidebar, so
//! a person reads the same mark in both places, and the back control that returns
//! to the front desk stands 8 points left of that tile.
//!
//! The second line names the conversation: `Orchestrator · Claude Code on this
//! computer` for a workspace, and the job of the front desk for the front desk.
//! The state of the conversation stands in the chip at the right edge, in the form
//! of the chip of a card, so a person reads the same mark in both places.
//!
//! The cost of the ended turns stands left of the chip while it is above zero.
//! A header that is too narrow for the name, the cost and the chip cuts the name
//! with an ellipsis first, and then hides the cost. The chip stays.

use std::sync::Arc;

use egui::{CursorIcon, Galley, Rect, Sense, Stroke, Ui, WidgetInfo, pos2, vec2};
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
/// The side of the back control, its corner radius, the air between it and the
/// tile, and the size of the chevron that it draws: 5 points wide and 9 high,
/// centred on the control.
const BACK_SIDE: f32 = 22.0;
const BACK_RADIUS: u8 = 7;
const BACK_GAP: f32 = 8.0;
const BACK_CHEVRON_WIDTH: f32 = 5.0;
const BACK_CHEVRON_HEIGHT: f32 = 9.0;
const BACK_STROKE: f32 = 1.5;
/// The name of the back control, which a screen reader reads and a pointer reads
/// as its hover text.
const BACK_NAME: &str = "Front desk";
/// The widget id of the back control.
const BACK_ID: &str = "orchestrator_dock_back";
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
/// The second line of a header of a workspace: the program, without its state,
/// which the chip names.
pub(super) const PROGRAM_LINE: &str = "Orchestrator \u{b7} Claude Code on this computer";
/// The first line of a header of the front desk, and the second one, which names
/// what the front desk does.
pub(super) const FRONT_DESK_TITLE: &str = "Front desk";
pub(super) const FRONT_DESK_LINE: &str = "Starts and watches the work items";

/// What the header of the dock names: the front desk, which belongs to no
/// workspace, one workspace of the board, or one mirrored session of another
/// computer.
#[derive(Clone)]
pub(super) enum Form<'a> {
    FrontDesk,
    Workspace(&'a Workspace),
    /// A session of another computer: its title, and the line that names the program
    /// and the computer.
    Mirror {
        title: String,
        line: String,
    },
}

impl Form<'_> {
    /// The first line of the header.
    fn title(&self) -> &str {
        match self {
            Self::FrontDesk => FRONT_DESK_TITLE,
            Self::Workspace(workspace) => workspace.name.as_str(),
            Self::Mirror { title, .. } => title,
        }
    }

    /// The second line of the header, under the first one.
    fn line(&self) -> &str {
        match self {
            Self::FrontDesk => FRONT_DESK_LINE,
            Self::Workspace(_) => PROGRAM_LINE,
            Self::Mirror { line, .. } => line,
        }
    }

    /// The room that the marks at the left take before the two lines: the back
    /// control and the tile of a workspace, and nothing for the front desk or for a
    /// mirrored session, which has no back control of its own.
    fn leading(&self) -> f32 {
        match self {
            Self::FrontDesk | Self::Mirror { .. } => 0.0,
            Self::Workspace(_) => BACK_SIDE + BACK_GAP + TILE_SIDE + TILE_GAP,
        }
    }
}

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

/// What the header names of the conversation that the dock shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct State {
    /// The chip of the state of the conversation.
    pub(super) chip: Chip,
    /// The cost of the ended turns of the conversation.
    pub(super) cost_usd_total: f64,
}

impl State {
    /// The state of one mirrored session: the chip reads `live` while a process runs
    /// it and `idle` otherwise, and the mirror has no cost.
    pub(super) fn mirrored(live: bool) -> Self {
        Self {
            chip: Chip {
                word: if live { "live" } else { "idle" },
                dot: if live {
                    ChipDot::Filled(theme::PALETTE_GREEN())
                } else {
                    ChipDot::Hollow
                },
            },
            cost_usd_total: 0.0,
        }
    }

    /// The state of the conversation of `session`, or the state of a
    /// conversation that started no program yet.
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

/// The rectangles of the marks of the header, which the tests of the layout read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) struct Parts {
    /// The back control, while the header shows a workspace.
    pub(in crate::app) back: Option<Rect>,
    /// The tile of the workspace, while the header shows one.
    pub(in crate::app) tile: Option<Rect>,
    /// The left edge of the first and of the second line.
    pub(in crate::app) text_left: f32,
}

/// The rectangles of the marks of the header of `rect`.
pub(super) fn parts(rect: Rect, form: &Form<'_>) -> Parts {
    let back = matches!(form, Form::Workspace(_)).then(|| {
        Rect::from_min_size(
            pos2(rect.min.x + PADDING_X, rect.center().y - BACK_SIDE / 2.0),
            vec2(BACK_SIDE, BACK_SIDE),
        )
    });
    let tile = back.map(|back| {
        Rect::from_min_size(
            pos2(back.max.x + BACK_GAP, rect.center().y - TILE_SIDE / 2.0),
            vec2(TILE_SIDE, TILE_SIDE),
        )
    });
    let text_left = tile.map_or(rect.min.x + PADDING_X, |tile| tile.max.x + TILE_GAP);
    Parts { back, tile, text_left }
}

/// The three galleys of the header.
struct Lines {
    name: Arc<Galley>,
    state: Arc<Galley>,
    cost: Option<Arc<Galley>>,
}

/// The width of the chip of `state`, which stands at the right edge.
fn chip_width(ui: &Ui, state: State) -> f32 {
    chip::words_width(ui.painter(), state.chip.word) + TEXT_CHIP_GAP
}

/// The galleys of the header of `width` points: the name of `form`, the line
/// under it, the cost and the chip at the right.
fn lines(ui: &Ui, width: f32, form: &Form<'_>, state: State) -> Lines {
    let column = (width - 2.0 * PADDING_X - form.leading()).max(0.0);
    let name_column = (column - chip_width(ui, state)).max(0.0);
    let mut cost = cost_of(ui, state.cost_usd_total, name_column);
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
    let name = text::line(ui, form.title(), NAME_SIZE, Weight::Semibold, theme::FG(), name_width);
    // The second line keeps the whole column: the chip and the cost stand on the
    // row of the name, so this line never meets them.
    let state = text::line(ui, form.line(), STATE_SIZE, Weight::Regular, theme::FG_SOFT(), column);
    Lines { name, state, cost }
}

/// The total of the conversation left of the chip, while it is above zero.
fn cost_of(ui: &Ui, cost_usd_total: f64, max_width: f32) -> Option<Arc<Galley>> {
    (cost_usd_total > 0.0)
        .then(|| text::mono_line(ui, &cost_words(cost_usd_total), COST_SIZE, theme::FG_SOFT(), max_width))
}

/// The words of a cost above zero. A cost that rounds to no cent is not free, so
/// it reads `<0.01 USD` and never `0.00 USD`.
pub(super) fn cost_words(cost_usd_total: f64) -> String {
    if cost_usd_total < 0.005 {
        "<0.01 USD".to_owned()
    } else {
        format!("{cost_usd_total:.2} USD")
    }
}

/// The height of the header of `form` at `width` points, which the dock works out
/// before it draws the header.
pub(super) fn height(ui: &Ui, width: f32, form: &Form<'_>, state: State) -> f32 {
    let lines = lines(ui, width, form, state);
    let text_height = lines.name.size().y + LINE_GAP + lines.state.size().y;
    let chip_height = chip::height();
    (text_height.max(TILE_SIDE).max(chip_height) + 2.0 * PADDING_Y).ceil()
}

/// Draws the header of `form` in `rect`, its line under the name, the cost left of
/// the chip and the chip at the right edge. It answers whether the person pressed
/// the back control, which the front desk has and a workspace shows.
pub(super) fn render(ui: &Ui, rect: Rect, form: &Form<'_>, state: State) -> bool {
    let lines = lines(ui, rect.width(), form, state);
    let name_height = lines.name.size().y;
    let text_height = name_height + LINE_GAP + lines.state.size().y;
    let parts = parts(rect, form);

    let text_left = parts.text_left;
    let text_top = rect.center().y - text_height / 2.0;
    if let (Some(tile), Form::Workspace(workspace)) = (parts.tile, form) {
        // The dock shows the workspace of this form, so its tile is the active
        // one.
        let accent = theme::workspace_accent(workspace.color_idx);
        rows::paint_workspace_tile(ui, tile, &workspace.name, accent, true);
    }

    let painter = ui.painter();
    painter.galley(pos2(text_left, text_top), Arc::clone(&lines.name), theme::FG());
    let state_top = text_top + name_height + LINE_GAP;
    painter.galley(pos2(text_left, state_top), Arc::clone(&lines.state), theme::FG_SOFT());

    let width = chip::words_width(painter, state.chip.word);
    // The chip and the cost stand on the row of the name, so the line under them
    // keeps the whole width of the header.
    let chip_center = text_top + name_height / 2.0;
    let chip = Rect::from_min_size(
        pos2(rect.max.x - PADDING_X - width, chip_center - chip::height() / 2.0),
        vec2(width, chip::height()),
    );
    if let Some(cost) = lines.cost {
        // The cost and the chip share one centre line at the right edge.
        let cost_rect = Rect::from_min_size(
            pos2(
                chip.min.x - COST_GAP - cost.size().x,
                chip.center().y - cost.size().y / 2.0,
            ),
            cost.size(),
        );
        painter.galley(cost_rect.min, cost, theme::FG_SOFT());
    }
    chip::paint_words(painter, chip, state.chip.word, state.chip.dot);
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
    );
    parts.back.is_some_and(|back| back_control(ui, back))
}

/// Draws the back control on `rect` and answers whether the person pressed it. It
/// has no fill at rest, and the hover fills it with `PANEL_BG_ALT` and turns its
/// chevron to `FG`.
fn back_control(ui: &Ui, rect: Rect) -> bool {
    let response = ui.interact(rect, ui.make_persistent_id(BACK_ID), Sense::click());
    let hovered = response.hovered();
    let painter = ui.painter();
    if hovered {
        painter.rect_filled(rect, BACK_RADIUS, theme::PANEL_BG_ALT());
    }
    let color = if hovered { theme::FG() } else { theme::FG_SOFT() };
    let stroke = Stroke::new(BACK_STROKE, color);
    let centre = rect.center();
    let tip = pos2(centre.x - BACK_CHEVRON_WIDTH / 2.0, centre.y);
    painter.line_segment(
        [
            pos2(tip.x + BACK_CHEVRON_WIDTH, centre.y - BACK_CHEVRON_HEIGHT / 2.0),
            tip,
        ],
        stroke,
    );
    painter.line_segment(
        [
            tip,
            pos2(tip.x + BACK_CHEVRON_WIDTH, centre.y + BACK_CHEVRON_HEIGHT / 2.0),
        ],
        stroke,
    );
    response.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, BACK_NAME));
    let response = response
        .on_hover_text(BACK_NAME)
        .on_hover_cursor(CursorIcon::PointingHand);
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::cost_words;

    #[test]
    fn a_cost_under_one_cent_is_not_shown_as_no_cost() {
        assert_eq!(cost_words(0.0004), "<0.01 USD");
        assert_eq!(cost_words(0.0049), "<0.01 USD");
        assert_eq!(cost_words(0.005), "0.01 USD");
        assert_eq!(cost_words(1.239), "1.24 USD");
    }
}
