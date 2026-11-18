//! The follow panels of a board: panels that show a terminal of another computer.

use crate::error::Result;
use crate::panel::{Panel, PanelId, PanelOptions};
use crate::terminal::FollowSpec;
use crate::workspace::WorkspaceId;

use super::{Board, PANEL_CHROME_PAD, PANEL_CHROME_TITLEBAR};

/// The width that the scrollbar of a terminal and its gap take beside the grid.
const SCROLLBAR_SPAN: f32 = 20.0;

impl Board {
    /// Opens a panel in `workspace` that follows the terminal of `spec`. The
    /// panel gives its feed with [`Panel::follow_feed`] and its `spec` with
    /// [`Panel::follow_spec`]. No session saves it and no start restores it.
    ///
    /// `cell` is the width and the height of one cell of the terminal font in
    /// points, which the terminal widget measures. The body of the new panel
    /// holds the grid of `spec` in that cell size exactly, so the first key of
    /// the person asks the program of the owner for no other grid.
    ///
    /// # Errors
    ///
    /// Returns why the terminal of the follower cannot start.
    pub fn create_follow_panel(&mut self, workspace: WorkspaceId, spec: FollowSpec, cell: [f32; 2]) -> Result<PanelId> {
        let options = PanelOptions {
            size: Some(follow_panel_size(spec.cols, spec.rows, cell)),
            ..PanelOptions::default()
        };
        self.create_panel_with(options, workspace, |id, workspace, _| {
            Panel::follow(id, workspace, spec)
        })
    }
}

/// The size of a panel whose body holds a grid of `cols` by `rows` cells of
/// `cell` points. A grid above 240 by 80 cells opens at that size, and the panel
/// clips the rest.
fn follow_panel_size(cols: u16, rows: u16, cell: [f32; 2]) -> [f32; 2] {
    // A body that stops one point before the start of the next cell reads back
    // as `cells` cells, whatever the float error of the division by a cell.
    let body = |cells: u16, side: f32| (f32::from(cells) + 1.0) * side - 1.0;
    [
        body(cols.clamp(20, 240), cell[0]) + SCROLLBAR_SPAN + 2.0 * PANEL_CHROME_PAD,
        body(rows.clamp(4, 80), cell[1]) + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD,
    ]
}
