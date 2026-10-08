//! Clouds in a cast: a workspace source shows all of its clouds, a cloud source one of
//! them. A cloud's frame, header, body and open drawer are part of the picture, and its
//! own chrome does not count as something covering it.
use super::super::HorizonApp;
use egui::{Context, LayerId, Rect};
use horizon_core::WorkspaceId;

/// The clouds of a workspace that a cast shows.
#[derive(Clone, Copy)]
pub(super) enum Clouds<'a> {
    /// Every cloud in the workspace.
    All,
    /// The deployed cloud with this cloud ID.
    One(&'a str),
}

impl HorizonApp {
    /// Screen rectangles of the selected clouds in `workspace`. A cloud only partly in
    /// view cannot be cast whole, so it refuses the source like a panel does.
    pub(super) fn cast_cloud_rects(
        &self,
        workspace: WorkspaceId,
        clouds: Clouds<'_>,
        ctx: &Context,
    ) -> Result<Vec<Rect>, String> {
        #[cfg(feature = "cloud-workspaces")]
        {
            let canvas = self.canvas_rect(ctx);
            let mut rects = Vec::new();
            for group in self.cast_clouds(workspace, clouds) {
                let rect = self.cast_cloud_screen_rect(group, ctx);
                if !canvas.contains_rect(rect) {
                    return Err("Fit the entire source into view before casting".into());
                }
                rects.push(rect);
            }
            Ok(rects)
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, clouds, ctx);
            Ok(Vec::new())
        }
    }

    /// Whether each selected cloud was last drawn where it stands now, so a capture does
    /// not start while the canvas still moves it.
    pub(super) fn cast_clouds_settled(&self, workspace: WorkspaceId, clouds: Clouds<'_>, ctx: &Context) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            let transform = crate::app::view::canvas_scene_transform(self.canvas_rect(ctx), self.canvas_view);
            self.cast_clouds(workspace, clouds).all(|group| {
                let (min, max) = group.bounds();
                let current = transform * Rect::from_min_max(egui::Pos2::from(min), egui::Pos2::from(max));
                let layer = LayerId::new(egui::Order::Background, egui::Id::new(("cloud-frame", group.issue)));
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
            let _ = (workspace, clouds, ctx);
            true
        }
    }

    /// Whether `layer` draws part of a selected cloud in `workspace`: its frame, header,
    /// body, drawer, corner grip or the owner badge of one of its panels.
    pub(super) fn cast_cloud_layer(&self, workspace: WorkspaceId, clouds: Clouds<'_>, layer: LayerId) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cast_clouds(workspace, clouds).any(|group| {
                CLOUD_LAYERS
                    .iter()
                    .any(|name| layer.id == egui::Id::new((*name, group.issue)))
                    || self.board.panels.iter().any(|panel| {
                        group.panels.contains(&panel.local_id) && layer.id == egui::Id::new(("cloud-owner", panel.id.0))
                    })
            })
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, clouds, layer);
            false
        }
    }

    /// The local IDs of the panels in the deployed cloud `id` of `workspace`, or `None`
    /// when the workspace has no such cloud.
    pub(super) fn cast_cloud_members(&self, workspace: WorkspaceId, id: &str) -> Option<Vec<String>> {
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cast_clouds(workspace, Clouds::One(id))
                .next()
                .map(|group| group.panels.clone())
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, id);
            None
        }
    }

    /// Whether more than one cloud, in any workspace, carries the cloud ID `id`.
    pub(super) fn cast_cloud_duplicated(&self, id: &str) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cloud_prototype
                .groups
                .0
                .iter()
                .filter(|group| group.remote.as_ref().is_some_and(|launch| launch.id == id))
                .count()
                > 1
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = id;
            false
        }
    }

    /// Whether a cloud of `workspace` other than `id` lies over `bounds`.
    pub(super) fn cast_other_cloud_over(&self, workspace: WorkspaceId, id: &str, bounds: Rect, ctx: &Context) -> bool {
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cast_clouds(workspace, Clouds::All)
                .filter(|group| group.remote.as_ref().is_none_or(|launch| launch.id != id))
                .any(|group| self.cast_cloud_screen_rect(group, ctx).intersects(bounds))
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = (workspace, id, bounds, ctx);
            false
        }
    }

    /// The deployed clouds of `workspace` that can be cast, as cloud ID and title.
    pub(super) fn cast_cloud_sources(&self, workspace: WorkspaceId) -> Vec<(String, String)> {
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cast_clouds(workspace, Clouds::All)
                .filter_map(|group| {
                    group
                        .remote
                        .as_ref()
                        .map(|launch| (launch.id.clone(), group.title.clone()))
                })
                .collect()
        }
        #[cfg(not(feature = "cloud-workspaces"))]
        {
            let _ = workspace;
            Vec::new()
        }
    }

    /// Opens the Cast picker on the deployed cloud `issue`, from its Manage tab.
    #[cfg(feature = "cloud-workspaces")]
    pub(in crate::app) fn open_cloud_cast_picker(&mut self, issue: u32, ctx: &Context) {
        let Some(group) = self.cloud_prototype.groups.0.iter().find(|group| group.issue == issue) else {
            return;
        };
        let (Some(launch), Some(workspace)) = (
            group.remote.as_ref(),
            self.board.workspace_id_by_local_id(&group.workspace),
        ) else {
            return;
        };
        let source = horizon_core::browser::manifest::cast::CastSource::Cloud { id: launch.id.clone() };
        self.casting
            .toggle_picker(super::Anchor::Cloud(issue), workspace, source, ctx);
    }

    #[cfg(feature = "cloud-workspaces")]
    fn cast_clouds<'a>(
        &'a self,
        workspace: WorkspaceId,
        clouds: Clouds<'a>,
    ) -> impl Iterator<Item = &'a horizon_core::cloud_panel::CloudGroup> {
        let local = self.board.workspace(workspace).map(|value| value.local_id.clone());
        self.cloud_prototype.groups.0.iter().filter(move |group| {
            local.as_deref() == Some(group.workspace.as_str())
                && match clouds {
                    Clouds::All => true,
                    Clouds::One(id) => group.remote.as_ref().is_some_and(|launch| launch.id == id),
                }
        })
    }

    /// The card on screen: frame, header, body and an open drawer, which can reach below
    /// a short cloud.
    #[cfg(feature = "cloud-workspaces")]
    fn cast_cloud_screen_rect(&self, group: &horizon_core::cloud_panel::CloudGroup, ctx: &Context) -> Rect {
        let transform = crate::app::view::canvas_scene_transform(self.canvas_rect(ctx), self.canvas_view);
        let (min, max) = group.overview_bounds();
        let mut card = Rect::from_min_max(egui::Pos2::from(min), egui::Pos2::from(max));
        if let Some(drawer) = self.cloud_drawer_rect(group) {
            card = card.union(drawer);
        }
        transform * card
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
