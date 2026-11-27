//! The header of the dock: the tile and the name of the active workspace, the
//! state of its conversation, and the cost of that conversation at the right.
//!
//! The tile is the tile of the workspace row of the sidebar, drawn at the side
//! of the header, so a person reads the same mark in both places.
//!
//! The second line names the program and the state of the conversation, as the
//! dock of `super::session` reads them: `Orchestrator · Claude Code · ready`.
//! The cost of the ended turns stands at the right edge of that line while it is
//! above zero.

use std::sync::Arc;

use egui::{Galley, Rect, Stroke, Ui, pos2, vec2};
use horizon_core::Workspace;

use crate::app::sidebar::rows;
use crate::theme;
use crate::typeface::Weight;

use super::session::Session;
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
/// The cost at the right edge of the header, and the air before it.
const COST_SIZE: f32 = 11.0;
const COST_GAP: f32 = 8.0;
/// The name line of a window with no active workspace.
pub(super) const NO_WORKSPACE_LINE: &str = "No work item is open";

/// What the header names of the conversation of the active workspace.
#[derive(Clone, Copy)]
pub(super) struct State<'a> {
    /// The second line: the program and the state of the conversation.
    pub(super) line: &'a str,
    /// The cost of the ended turns of the conversation.
    pub(super) cost_usd_total: f64,
}

impl<'a> State<'a> {
    /// The state of the conversation of `session`, or the state of a workspace
    /// that has no conversation yet.
    pub(super) fn of(session: Option<&'a Session>) -> Self {
        session.map_or(
            Self {
                line: super::session::Phase::NotConnected.line(false),
                cost_usd_total: 0.0,
            },
            |session| Self {
                line: session.state_line(),
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

/// The galleys of the header of `width` points: the name of the active
/// `workspace`, the line of its conversation under it, and the cost at the right.
fn lines(ui: &Ui, width: f32, workspace: Option<&Workspace>, state: Option<State<'_>>) -> Lines {
    let text_width = (width - 2.0 * PADDING_X - tile_space(workspace)).max(0.0);
    let name = workspace.map_or(NO_WORKSPACE_LINE, |workspace| workspace.name.as_str());
    let name = text::line(ui, name, NAME_SIZE, Weight::Semibold, theme::FG(), text_width);
    let cost = state.and_then(|state| cost(ui, state.cost_usd_total, text_width));
    let state_width = cost
        .as_ref()
        .map_or(text_width, |cost| (text_width - cost.size().x - COST_GAP).max(1.0));
    let state = state.map(|state| {
        text::line(
            ui,
            state.line,
            STATE_SIZE,
            Weight::Regular,
            theme::FG_SOFT(),
            state_width,
        )
    });
    Lines { name, state, cost }
}

/// The total of the conversation at the right of the header, while it is above
/// zero.
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
pub(super) fn height(ui: &Ui, width: f32, workspace: Option<&Workspace>, state: Option<State<'_>>) -> f32 {
    let lines = lines(ui, width, workspace, state);
    let text_height = lines.state.as_ref().map_or(lines.name.size().y, |state| {
        lines.name.size().y + LINE_GAP + state.size().y
    });
    (text_height.max(TILE_SIDE) + 2.0 * PADDING_Y).ceil()
}

/// Draws the header of the dock for the active `workspace` in `rect`, its line
/// under the name and the cost at the right of it.
pub(super) fn render(ui: &Ui, rect: Rect, workspace: Option<&Workspace>, state: Option<State<'_>>) {
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
            pos2(tile_left, rect.center().y - TILE_SIDE / 2.0),
            vec2(TILE_SIDE, TILE_SIDE),
        );
        // The dock shows the active workspace, so its tile is the active one.
        let accent = theme::workspace_accent(workspace.color_idx);
        rows::paint_workspace_tile(ui, tile, &workspace.name, accent, true);
    }

    let painter = ui.painter();
    painter.galley(pos2(text_left, text_top), lines.name, theme::FG());
    let state_top = text_top + name_height + LINE_GAP;
    if let Some(state) = lines.state {
        painter.galley(pos2(text_left, state_top), state, theme::FG_SOFT());
    }
    if let Some(cost) = lines.cost {
        painter.galley(
            pos2(rect.max.x - PADDING_X - cost.size().x, state_top),
            cost,
            theme::FG_SOFT(),
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
    );
}
