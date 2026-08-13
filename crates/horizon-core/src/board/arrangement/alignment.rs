use crate::layout::{WORKSPACE_GAP, WS_EMPTY_FRAME_SIZE, WS_FRAME_PAD, WS_FRAME_TOP_EXTRA};
use crate::workspace::WorkspaceId;

use super::super::Board;

const ALIGNMENT_POSITION_ABSOLUTE_TOLERANCE: f32 = 0.01;
// At large canvas coordinates the relative term covers roughly two f32 ULPs,
// preventing a recomputed row from producing a second persistence-only pass.
const ALIGNMENT_POSITION_RELATIVE_TOLERANCE: f32 = 2.0 * f32::EPSILON;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspaceAlignment {
    pub leftmost_workspace: WorkspaceId,
    pub positions_changed: bool,
}

fn alignment_positions_match(left: f32, right: f32) -> bool {
    if !left.is_finite() || !right.is_finite() {
        return false;
    }

    let magnitude = left.abs().max(right.abs()).max(1.0);
    let tolerance = ALIGNMENT_POSITION_ABSOLUTE_TOLERANCE.max(ALIGNMENT_POSITION_RELATIVE_TOLERANCE * magnitude);
    (left - right).abs() <= tolerance
}

fn translated_position_is_finite(position: [f32; 2], delta: [f32; 2]) -> bool {
    [position[0] + delta[0], position[1] + delta[1]]
        .iter()
        .all(|component| component.is_finite())
}

fn translated_workspace_frame_is_finite(
    board: &Board,
    workspace_id: WorkspaceId,
    frame: [f32; 4],
    delta: [f32; 2],
) -> bool {
    board
        .workspace(workspace_id)
        .is_some_and(|workspace| translated_position_is_finite(workspace.position, delta))
        && [
            frame[0] + delta[0],
            frame[1] + delta[1],
            frame[2] + delta[0],
            frame[3] + delta[1],
        ]
        .iter()
        .all(|component| component.is_finite())
}

impl Board {
    /// Align the selected workspaces side by side in a horizontal row,
    /// sorted by their current x position, with consistent vertical
    /// alignment and [`WORKSPACE_GAP`] spacing between frames. Returns the
    /// alignment anchor and whether any positions changed, or `None` when
    /// fewer than two selected workspaces exist or their geometry is invalid.
    pub fn align_workspaces_horizontally(&mut self, workspace_ids: &[WorkspaceId]) -> Option<WorkspaceAlignment> {
        if workspace_ids.len() < 2 {
            return None;
        }

        let bounds_map = self.workspace_bounds_map();
        let mut entries: Vec<(WorkspaceId, [f32; 4])> = workspace_ids
            .iter()
            .filter_map(|workspace_id| {
                let workspace = self.workspace(*workspace_id)?;
                let frame = if let Some((min, max)) = bounds_map.get(&workspace.id) {
                    [
                        min[0] - WS_FRAME_PAD,
                        min[1] - WS_FRAME_PAD - WS_FRAME_TOP_EXTRA,
                        max[0] + WS_FRAME_PAD,
                        max[1] + WS_FRAME_PAD,
                    ]
                } else {
                    let position = workspace.position;
                    [
                        position[0],
                        position[1],
                        position[0] + WS_EMPTY_FRAME_SIZE[0],
                        position[1] + WS_EMPTY_FRAME_SIZE[1],
                    ]
                };
                Some((workspace.id, frame))
            })
            .collect();

        if entries.len() < 2 {
            return None;
        }

        let invalid_workspace = entries.iter().find_map(|(workspace_id, frame)| {
            let workspace = self.workspace(*workspace_id)?;
            let workspace_is_finite = workspace.position.iter().all(|component| component.is_finite());
            let panels_are_valid = self
                .panels
                .iter()
                .filter(|panel| panel.workspace_id == *workspace_id)
                .all(|panel| {
                    panel.layout.position.iter().all(|component| component.is_finite())
                        && panel
                            .layout
                            .size
                            .iter()
                            .all(|component| component.is_finite() && *component > 0.0)
                });
            (!workspace_is_finite || !panels_are_valid || !frame.iter().all(|component| component.is_finite()))
                .then_some(*workspace_id)
        });
        if let Some(workspace_id) = invalid_workspace {
            tracing::warn!(
                workspace_id = workspace_id.0,
                "workspace alignment skipped because geometry is invalid"
            );
            return None;
        }

        entries.sort_by(|left, right| left.1[0].total_cmp(&right.1[0]));

        let leftmost_workspace = entries[0].0;
        let anchor_y = entries[0].1[1];
        let mut cursor_x = entries[0].1[0];
        let mut translations = Vec::with_capacity(entries.len());

        for (workspace_id, frame) in &entries {
            let frame_width = frame[2] - frame[0];
            let delta = [cursor_x - frame[0], anchor_y - frame[1]];
            let next_cursor_x = cursor_x + frame_width + WORKSPACE_GAP;
            if !frame_width.is_finite()
                || !delta.iter().all(|component| component.is_finite())
                || !next_cursor_x.is_finite()
            {
                tracing::warn!("workspace alignment skipped because its translation plan is non-finite");
                return None;
            }

            if !alignment_positions_match(cursor_x, frame[0]) || !alignment_positions_match(anchor_y, frame[1]) {
                if !translated_workspace_frame_is_finite(self, *workspace_id, *frame, delta) {
                    tracing::warn!(
                        workspace_id = workspace_id.0,
                        "workspace alignment skipped because translated geometry would be non-finite"
                    );
                    return None;
                }
                translations.push((*workspace_id, delta));
            }
            cursor_x = next_cursor_x;
        }

        let positions_changed = translations.into_iter().fold(false, |changed, (workspace_id, delta)| {
            self.translate_workspace(workspace_id, delta) || changed
        });

        Some(WorkspaceAlignment {
            leftmost_workspace,
            positions_changed,
        })
    }
}
