//! The view the form sits in. The form sits on the workspace that holds its draft, so a canvas
//! that changes, from a window resize or from the sidebar, leaves the form where the smaller
//! canvas put it: against an edge, with the new room empty beside it. The workspace comes back
//! to the middle of the canvas instead, which is where the form opens.
use super::super::{HorizonApp, Production};
use egui::{Context, Rect};

/// Whether two canvas rects are the same within a pixel, so that a rounding difference of the
/// layout is no resize.
fn same_canvas(before: Rect, now: Rect) -> bool {
    [
        before.min.x - now.min.x,
        before.min.y - now.min.y,
        before.max.x - now.max.x,
        before.max.y - now.max.y,
    ]
    .iter()
    .all(|difference| difference.abs() < 1.0)
}

impl Production {
    /// Records the canvas of this frame and says whether it changed. Nothing is recorded while
    /// the form is closed, so the first frame of an opening form moves nothing.
    fn record_canvas(&mut self, now: Rect) -> bool {
        self.creation_canvas
            .replace(now)
            .is_some_and(|before| !same_canvas(before, now))
    }

    /// Forgets the canvas, so that a form which opens again starts from the canvas it opens in.
    pub(in crate::app::cloud_panel::production) fn forget_creation_canvas(&mut self) {
        self.creation_canvas = None;
    }
}

impl HorizonApp {
    /// Brings the draft's workspace back to the middle of the canvas when the canvas changes,
    /// with the zoom the person chose. A pan of their own is kept: only a change of the canvas
    /// itself moves the view.
    ///
    /// Only the workspace the person works in moves. The form stays open while they go to
    /// another workspace, and a resize there must not pull them back to the draft. Nothing takes
    /// focus either, so a panel they type in keeps the keyboard.
    pub(in crate::app::cloud_panel::production) fn follow_cloud_creation_view(&mut self, ctx: &Context) {
        let canvas = self.canvas_rect(ctx);
        if !self.cloud_prototype.production.record_canvas(canvas) {
            return;
        }
        let Some(workspace) = self
            .cloud_prototype
            .production
            .creation_workspace()
            .as_deref()
            .and_then(|local| self.board.workspace_id_by_local_id(local))
        else {
            return;
        };
        if self.board.active_workspace != Some(workspace) || self.workspace_is_detached(workspace) {
            return;
        }
        self.center_workspace_in_rect(workspace, canvas);
    }
}
