//! The follow panels of a board: panels that show a terminal of another computer.

use crate::error::Result;
use crate::panel::{DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, Panel, PanelId, PanelOptions};
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
    /// # Errors
    ///
    /// Returns why the terminal of the follower cannot start.
    pub fn create_follow_panel(&mut self, workspace: WorkspaceId, spec: FollowSpec) -> Result<PanelId> {
        let options = PanelOptions {
            size: Some(follow_panel_size(spec.cols, spec.rows)),
            ..PanelOptions::default()
        };
        self.create_panel_with(options, workspace, |id, workspace, _| {
            Panel::follow(id, workspace, spec)
        })
    }
}

/// The size of a panel whose body holds a grid of `cols` by `rows` cells of the
/// default size. A grid above 240 by 80 cells opens at that size, and the panel
/// clips the rest.
fn follow_panel_size(cols: u16, rows: u16) -> [f32; 2] {
    let width = f32::from(cols.clamp(20, 240)) * f32::from(DEFAULT_CELL_WIDTH);
    let height = f32::from(rows.clamp(4, 80)) * f32::from(DEFAULT_CELL_HEIGHT);
    [
        width + SCROLLBAR_SPAN + 2.0 * PANEL_CHROME_PAD,
        height + PANEL_CHROME_TITLEBAR + 2.0 * PANEL_CHROME_PAD,
    ]
}
