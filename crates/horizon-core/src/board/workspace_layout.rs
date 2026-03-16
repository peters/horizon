use serde::{Deserialize, Serialize};

use crate::layout::{TILE_GAP, WS_INNER_PAD, ceil_sqrt_usize, usize_to_f32};
use crate::panel::DEFAULT_PANEL_SIZE;

pub(crate) const STACK_OFFSET_X: f32 = 16.0;
pub(crate) const STACK_OFFSET_Y: f32 = 20.0;
pub(crate) const CASCADE_OFFSET_X: f32 = 40.0;
pub(crate) const CASCADE_OFFSET_Y: f32 = 30.0;

pub(crate) fn vec2_eq(left: [f32; 2], right: [f32; 2]) -> bool {
    (left[0] - right[0]).abs() <= f32::EPSILON && (left[1] - right[1]).abs() <= f32::EPSILON
}

/// Predefined layout arrangements for panels inside a workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceLayout {
    /// Single column, panels stacked top-to-bottom.
    Rows,
    /// Single row, panels side by side.
    Columns,
    /// Square-ish grid (auto columns).
    Grid,
    /// Layered pile with slight offsets to keep nearby panels accessible.
    Stack,
    /// Diagonal overlap that fans panels across the workspace.
    Cascade,
}

impl WorkspaceLayout {
    pub const ALL: [Self; 5] = [Self::Rows, Self::Columns, Self::Grid, Self::Stack, Self::Cascade];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rows => "Rows",
            Self::Columns => "Columns",
            Self::Grid => "Grid",
            Self::Stack => "Stack",
            Self::Cascade => "Cascade",
        }
    }
}

pub(crate) fn arranged_panel_layout(
    origin: [f32; 2],
    layout: WorkspaceLayout,
    index: usize,
    count: usize,
) -> ([f32; 2], [f32; 2]) {
    let panel_size = DEFAULT_PANEL_SIZE;

    match layout {
        WorkspaceLayout::Rows => {
            let x = origin[0] + WS_INNER_PAD;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * (panel_size[1] + TILE_GAP);
            ([x, y], panel_size)
        }
        WorkspaceLayout::Columns => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * (panel_size[0] + TILE_GAP);
            let y = origin[1] + WS_INNER_PAD;
            ([x, y], panel_size)
        }
        WorkspaceLayout::Grid => {
            let cols = ceil_sqrt_usize(count);
            let col = index % cols;
            let row = index / cols;
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(col) * (panel_size[0] + TILE_GAP);
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(row) * (panel_size[1] + TILE_GAP);
            ([x, y], panel_size)
        }
        WorkspaceLayout::Stack => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * STACK_OFFSET_X;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * STACK_OFFSET_Y;
            ([x, y], panel_size)
        }
        WorkspaceLayout::Cascade => {
            let x = origin[0] + WS_INNER_PAD + usize_to_f32(index) * CASCADE_OFFSET_X;
            let y = origin[1] + WS_INNER_PAD + usize_to_f32(index) * CASCADE_OFFSET_Y;
            ([x, y], panel_size)
        }
    }
}
