//! The header of the dock: the tile and the name of the active workspace, the
//! state of the orchestrator, and the line under them.
//!
//! The tile is the tile of the workspace row of the sidebar, drawn at the side
//! of the header, so a person reads the same mark in both places.

use std::sync::Arc;

use egui::{Galley, Rect, Stroke, Ui, pos2, vec2};
use horizon_core::Workspace;

use crate::app::sidebar::rows;
use crate::theme;
use crate::typeface::Weight;

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
/// The second line of the header. This part connects to no orchestrator yet, so
/// the line never names another state.
pub(super) const STATE_LINE: &str = "Orchestrator \u{b7} not connected";
/// The name line of a window with no active workspace.
pub(super) const NO_WORKSPACE_LINE: &str = "No work item is open";

/// The two lines of the header of `width` points: the name of the active
/// `workspace`, and the line under it when there is a workspace.
fn lines(ui: &Ui, width: f32, workspace: Option<&Workspace>) -> (Arc<Galley>, Option<Arc<Galley>>) {
    let tile_space = if workspace.is_some() { TILE_SIDE + TILE_GAP } else { 0.0 };
    let text_width = (width - 2.0 * PADDING_X - tile_space).max(0.0);
    let name = workspace.map_or(NO_WORKSPACE_LINE, |workspace| workspace.name.as_str());
    let name = text::line(ui, name, NAME_SIZE, Weight::Semibold, theme::FG(), text_width);
    let state = workspace.map(|_| {
        text::line(
            ui,
            STATE_LINE,
            STATE_SIZE,
            Weight::Regular,
            theme::FG_SOFT(),
            text_width,
        )
    });
    (name, state)
}

/// The height of the header of the active `workspace` at `width` points, which
/// the dock works out before it draws the header.
pub(super) fn height(ui: &Ui, width: f32, workspace: Option<&Workspace>) -> f32 {
    let (name, state) = lines(ui, width, workspace);
    let text_height = state
        .as_ref()
        .map_or(name.size().y, |state| name.size().y + LINE_GAP + state.size().y);
    (text_height.max(TILE_SIDE) + 2.0 * PADDING_Y).ceil()
}

/// Draws the header of the dock for the active `workspace` in `rect`, and the
/// line under it.
pub(super) fn render(ui: &Ui, rect: Rect, workspace: Option<&Workspace>) {
    let (name_galley, state_galley) = lines(ui, rect.width(), workspace);
    let name_height = name_galley.size().y;
    let text_height = state_galley
        .as_ref()
        .map_or(name_height, |state| name_height + LINE_GAP + state.size().y);
    let tile_space = if workspace.is_some() { TILE_SIDE + TILE_GAP } else { 0.0 };

    let tile_left = rect.min.x + PADDING_X;
    let text_left = tile_left + tile_space;
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
    painter.galley(pos2(text_left, text_top), name_galley, theme::FG());
    if let Some(state) = state_galley {
        painter.galley(
            pos2(text_left, text_top + name_height + LINE_GAP),
            state,
            theme::FG_SOFT(),
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
    );
}
