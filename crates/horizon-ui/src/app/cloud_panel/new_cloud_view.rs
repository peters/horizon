//! Where the view goes when a cloud is created: to its own workspace only, so the clouds
//! of other workspaces stay out of view and their parked terminals stay parked.
use super::{CloudGroups, HorizonApp};
use crate::app::MINIMAP_MARGIN;
use horizon_core::CanvasViewState;

/// `fitted_region` keeps this margin on each side of the region.
const MARGIN: f32 = 12.0;
/// Where the frame sits first in the free space of each axis, as a part of that space.
/// A tie keeps the earlier place, so a new workspace in the rightmost slot is aligned to
/// the left and centred from top to bottom.
const HORIZONTAL: [f32; 3] = [0.0, 0.5, 1.0];
const VERTICAL: [f32; 3] = [0.5, 0.0, 1.0];

/// A cloud this far past the edge of the canvas is out of view: a rectangle that only
/// touches the canvas still intersects it.
const CLEAR: f32 = 1.0;

/// The offsets of the frame in `slack` free pixels to try on one axis: the preferred
/// places, then each offset that puts a cloud just past an edge of the canvas. The shown
/// area changes linearly between these offsets, so its least value is at one of them.
/// `spans` are the clouds' extents from the frame's start, in pixels; `view` is the
/// canvas length.
fn offsets(slack: f32, preferred: [f32; 3], spans: &[(f32, f32)], view: f32) -> Vec<f32> {
    let edges = spans
        .iter()
        .flat_map(|&(start, end)| [-MARGIN - end - CLEAR, view - MARGIN - start + CLEAR]);
    preferred
        .iter()
        .map(|part| part * slack)
        .chain(edges.map(|offset| offset.clamp(0.0, slack)))
        .collect()
}

impl HorizonApp {
    /// A new cloud takes the view and the focus to its own workspace. A cloud in full
    /// screen keeps the view; the first panel then opens when the person comes to the
    /// workspace.
    pub(in crate::app::cloud_panel) fn show_new_cloud(&mut self, ctx: &egui::Context, id: u32) {
        if self.cloud_prototype.fullscreen.is_some() {
            return;
        }
        let Some((workspace, local)) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| {
                Some((
                    self.board.workspace_id_by_local_id(&group.workspace)?,
                    group.workspace.clone(),
                ))
            })
            .filter(|(workspace, _)| !self.workspace_is_detached(*workspace))
        else {
            return;
        };
        let Some((position, size)) = self.workspace_focus_frame(workspace) else {
            return;
        };
        self.board.focus_workspace(workspace);
        let frame = egui::Rect::from_min_size(position, size);
        let view = self.new_cloud_view(ctx, &local, frame);
        self.pan_target = None;
        if self.canvas_view != view {
            self.canvas_view = view;
            ctx.request_repaint();
        }
        self.mark_runtime_dirty();
    }

    /// Fits `frame` between the overview controls and the minimap without zooming in,
    /// and places it where the least of the other workspaces' clouds shows. On a window
    /// wider than the space between workspaces, part of a neighbour can still show.
    fn new_cloud_view(&self, ctx: &egui::Context, workspace: &str, frame: egui::Rect) -> CanvasViewState {
        let canvas = self.canvas_rect(ctx);
        let (controls, minimap) = self.cloud_overlays(ctx, canvas);
        let top = controls.bottom() + 12.0 - canvas.top();
        let bottom = minimap.map_or(canvas.bottom(), |rect| rect.top() - MINIMAP_MARGIN) - canvas.top();
        let region = [canvas.width(), (bottom - top).max(1.0)];
        let (min, max) = ([frame.min.x, frame.min.y], [frame.max.x, frame.max.y]);
        let zoom = CloudGroups::fitted_region(min, max, [0.0, top], region).zoom;
        let slack = [
            (region[0] - 2.0 * MARGIN - frame.width() * zoom).max(0.0),
            (region[1] - 2.0 * MARGIN - frame.height() * zoom).max(0.0),
        ];
        let others: Vec<egui::Rect> = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.workspace != workspace)
            .map(|group| {
                let (min, max) = group.overview_bounds();
                egui::Rect::from_min_max(min.into(), max.into())
            })
            .collect();
        let shown = |view: CanvasViewState| -> f32 {
            let transform = crate::app::view::canvas_scene_transform(canvas, view);
            others
                .iter()
                .map(|bounds| (transform * *bounds).intersect(canvas))
                .filter(egui::Rect::is_positive)
                .map(|visible| visible.area())
                .sum()
        };
        let spans = |axis: usize| -> Vec<(f32, f32)> {
            others
                .iter()
                .map(|bounds| {
                    (
                        (bounds.min[axis] - min[axis]) * zoom,
                        (bounds.max[axis] - min[axis]) * zoom,
                    )
                })
                .collect()
        };
        let mut best: Option<(f32, CanvasViewState)> = None;
        for x in offsets(slack[0], HORIZONTAL, &spans(0), canvas.width()) {
            for y in offsets(slack[1], VERTICAL, &spans(1), canvas.height() - top) {
                let view = CanvasViewState::new([MARGIN + x - min[0] * zoom, top + MARGIN + y - min[1] * zoom], zoom);
                let area = shown(view);
                if best.is_none_or(|(least, _)| area < least) {
                    best = Some((area, view));
                }
            }
        }
        best.map_or_else(
            || CloudGroups::fitted_region(min, max, [0.0, top], region),
            |(_, view)| view,
        )
    }
}
