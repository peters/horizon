//! Clouds in a workspace cast: their frames are part of the picture, and their own
//! chrome does not count as something covering it.
use super::super::HorizonApp;
use egui::{Context, Id, LayerId, Rect};
use horizon_core::WorkspaceId;

impl HorizonApp {
    /// Screen rectangles of the clouds drawn in `workspace`. A cloud only partly in view
    /// cannot be cast whole, so it refuses the source like a panel does.
    pub(super) fn cast_cloud_rects(&self, workspace: WorkspaceId, ctx: &Context) -> Result<Vec<Rect>, String> {
        #[cfg(feature = "cloud-workspaces")]
        {
            let Some(local) = self.board.workspace(workspace).map(|value| value.local_id.as_str()) else {
                return Ok(Vec::new());
            };
            let canvas = self.canvas_rect(ctx);
            let transform = crate::app::view::canvas_scene_transform(canvas, self.canvas_view);
            let mut rects = Vec::new();
            for group in self
                .cloud_prototype
                .groups
                .0
                .iter()
                .filter(|group| group.workspace == local)
            {
                let (min, max) = group.overview_bounds();
                let rect = transform * Rect::from_min_max(egui::Pos2::from(min), egui::Pos2::from(max));
                if !canvas.contains_rect(rect) {
                    return Err("Fit the entire source into view before casting".into());
                }
                rects.push(rect);
            }
            Ok(rects)
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, ctx);
            Ok(Vec::new())
        }
    }

    /// Whether each cloud in `workspace` was last drawn where it stands now, so a capture
    /// does not start while the canvas still moves it.
    pub(super) fn cast_clouds_settled(&self, workspace: WorkspaceId, ctx: &Context) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            let Some(local) = self.board.workspace(workspace).map(|value| value.local_id.as_str()) else {
                return true;
            };
            let transform = crate::app::view::canvas_scene_transform(self.canvas_rect(ctx), self.canvas_view);
            self.cloud_prototype
                .groups
                .0
                .iter()
                .filter(|group| group.workspace == local)
                .all(|group| {
                    let (min, max) = group.bounds();
                    let current = transform * Rect::from_min_max(egui::Pos2::from(min), egui::Pos2::from(max));
                    let layer = LayerId::new(egui::Order::Background, Id::new(("cloud-frame", group.issue)));
                    ctx.memory(|memory| {
                        memory.area_rect(layer.id).is_some_and(|area| {
                            let drawn = memory.to_global.get(&layer).copied().unwrap_or_default() * area;
                            (drawn.min - current.min).length() < 0.5 && (drawn.max - current.max).length() < 0.5
                        })
                    })
                })
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, ctx);
            true
        }
    }

    /// Whether `layer` draws part of a cloud in `workspace`: its frame, header, body,
    /// drawer, corner grip or the owner badge of one of its panels.
    pub(super) fn cast_cloud_layer(&self, workspace: WorkspaceId, layer: LayerId) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            let Some(local) = self.board.workspace(workspace).map(|value| value.local_id.as_str()) else {
                return false;
            };
            self.cloud_prototype
                .groups
                .0
                .iter()
                .filter(|group| group.workspace == local)
                .any(|group| {
                    CLOUD_LAYERS
                        .iter()
                        .any(|name| layer.id == Id::new((*name, group.issue)))
                        || self.board.panels.iter().any(|panel| {
                            group.panels.contains(&panel.local_id) && layer.id == Id::new(("cloud-owner", panel.id.0))
                        })
                })
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, layer);
            false
        }
    }
}

/// Areas a cloud draws, each keyed by `(name, issue)`.
#[cfg(feature = "cloud-workspaces")]
const CLOUD_LAYERS: [&str; 6] = [
    "cloud-frame",
    "cloud-header",
    "cloud-runtime",
    "cloud-drawer",
    "cloud-resize",
    "cloud-empty",
];
