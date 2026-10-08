//! Opt-in cloud-panel prototype: grouping of ordinary panels, not a new runtime.
mod capabilities;
mod fixture;
mod group;
pub mod park;
#[cfg(test)]
mod placement;
mod reordering;
mod resize;
mod selection;
mod slot;
pub use selection::{ChosenWorker, WorkerChoice};
pub use slot::SLOT_MIN_MEMBER;

pub use horizon_cloud::Connection as CloudConnection;
use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Board, PanelId, WorkspaceId, WorkspaceLayout};
pub use fixture::{PrototypeSnapshot, load, prepare_repository, save};
pub use horizon_cloud::{BrowserEngine, Capabilities, CloudConfig, Environment};

pub const HEADER: f32 = 84.0;
pub const PAD: f32 = 14.0;
pub const RUNTIME_WIDTH: f32 = 300.0;
pub const RUNTIME_HEIGHT: f32 = 740.0;
/// The status strip a production cloud adds under its title: one status line
/// and the stage track along the header's bottom edge.
pub const STATUS_HEIGHT: f32 = 34.0;
/// Room between a production cloud's header and the panels under it.
pub const CONTENT_GAP: f32 = 10.0;
/// Space the earlier summary card reserved above the sessions, and what its
/// disclosed controls added; saved clouds from then are moved up once.
const LEGACY_TOOLBAR_HEIGHT: f32 = 224.0;
const LEGACY_TOOLBAR_CONTROLS_HEIGHT: f32 = 76.0;
pub const CHILD_SIZE: [f32; 2] = [520.0, 500.0];
pub const CLOUDS: [(u32, &str); 5] = [
    (101, "Web preview"),
    (102, "Pair programming"),
    (103, "Full stack sandbox"),
    (104, "Cloud 4"),
    (105, "Cloud 5"),
];

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CloudGroup {
    #[serde(default)]
    pub remote: Option<CloudLaunch>,
    /// Same-worker siblings chosen in New cloud for `remote`, in layering order. Their
    /// checkout paths stay in this machine-local record, never in committed configuration.
    /// Omitted when none, so earlier records keep their encoding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<crate::cloud_runtime::siblings::Binding>,
    pub issue: u32,
    pub title: String,
    pub workspace: String,
    pub environment: Environment,
    pub cwd: PathBuf,
    pub position: [f32; 2],
    #[serde(default)]
    workspace_position: [f32; 2],
    pub size: [f32; 2],
    pub collapsed: bool,
    /// Present only in records saved while the summary card reserved space
    /// above the sessions: whether its controls were disclosed. Reconcile moves
    /// those sessions up under the status strip once, then drops it.
    #[serde(default, rename = "toolbar_expanded", skip_serializing)]
    legacy_toolbar: Option<bool>,
    /// `None` is manual placement. New clouds start with the default preset;
    /// a saved cloud without a stored layout stays manual.
    #[serde(default)]
    pub layout: Option<WorkspaceLayout>,
    pub panels: Vec<String>,
    /// Only panels hidden by collapse are revealed by expand.
    hidden: Vec<String>,
    /// Where this cloud stands among its workspace preset's slots while it has no
    /// members; a cloud with members stands where its first member does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
}

/// Fields reconciliation compares. Omits the remote launch payload so the
/// per-frame path does not clone it.
struct CloudGeometry {
    position: [f32; 2],
    size: [f32; 2],
    workspace_position: [f32; 2],
    collapsed: bool,
    /// Membership already matched `self.panels` when this snapshot was taken.
    panels_matched: bool,
    panel_count: usize,
}

impl CloudGeometry {
    fn differs_from(&self, group: &CloudGroup) -> bool {
        !self.panels_matched
            || self.panel_count != group.panels.len()
            || self.position.map(f32::to_bits) != group.position.map(f32::to_bits)
            || self.size.map(f32::to_bits) != group.size.map(f32::to_bits)
            || self.workspace_position.map(f32::to_bits) != group.workspace_position.map(f32::to_bits)
            || self.collapsed != group.collapsed
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CloudLaunch {
    #[serde(default)]
    pub deployment_started: bool,
    pub id: String,
    pub revision: String,
    pub profile_name: String,
    pub profile: horizon_cloud::Profile,
    /// Omitted when nothing was chosen, so earlier records keep their encoding.
    #[serde(default, skip_serializing_if = "Placement::is_default")]
    pub placement: Placement,
}

/// Where, and on which GPU types, a cloud may be placed, chosen when it is created. A
/// cloud's workspace stays where it is first placed, so this matters beyond the first start.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Placement {
    /// A label shown to people, such as `Europe`, for a chosen region or the region of
    /// a chosen data center; `None` when any data center will do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// The data centers to choose from; empty for the machine's `data_centers` setting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_centers: Vec<String>,
    /// GPU types to request, in this order; empty for the machine's `gpu_types` setting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpu_types: Vec<String>,
    /// Explicit CPU server types, narrowed to the machine's allowed types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu_types: Vec<String>,
}

impl Placement {
    /// Whether any allowed data center will do.
    #[must_use]
    pub fn is_any(&self) -> bool {
        self.data_centers.is_empty()
    }

    /// Whether nothing was chosen, so the machine settings apply unchanged.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.data_centers.is_empty() && self.gpu_types.is_empty() && self.cpu_types.is_empty()
    }

    /// This placement for a profile with or without a GPU: a GPU type chosen while the
    /// profile had one never reaches a CPU cloud.
    #[must_use]
    pub fn for_profile(&self, gpu: bool) -> Self {
        Self {
            gpu_types: if gpu { self.gpu_types.clone() } else { Vec::new() },
            cpu_types: if gpu { Vec::new() } else { self.cpu_types.clone() },
            ..self.clone()
        }
    }

    /// Replaces the machine's `data_centers` and `gpu_types` with this placement's
    /// choices, where it made one.
    pub fn apply(&self, data_centers: &mut Vec<String>, gpu_types: &mut Vec<String>) {
        if !self.data_centers.is_empty() {
            data_centers.clone_from(&self.data_centers);
        }
        if !self.gpu_types.is_empty() {
            gpu_types.clone_from(&self.gpu_types);
        }
    }
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct CloudGroups(pub Vec<CloudGroup>);

impl CloudGroups {
    /// Workspace-relative origin for a newly created cloud.
    ///
    /// Callers store the result on a group whose remembered workspace origin is
    /// zero; reconcile adds the live workspace position. The top edge sits below
    /// visible ordinary-panel bounds and clouds already in that workspace. Panel
    /// positions, cloud positions, and membership stay as they were.
    #[must_use]
    pub fn next_position(&self, workspace: &str, board: &Board) -> [f32; 2] {
        let destination = board.workspaces.iter().find(|item| item.local_id == workspace);
        let origin_y = destination.map_or(0.0, |item| item.position[1]);
        let panel_bottom = board
            .panels
            .iter()
            .filter(|panel| panel.visible && destination.is_some_and(|item| panel.workspace_id == item.id))
            .map(|panel| crate::board::panel_visual_rect(panel.layout.position, panel.layout.size)[3] - origin_y)
            .fold(80.0, f32::max);
        let bottom = self
            .0
            .iter()
            .filter(|group| group.workspace == workspace)
            .map(|group| {
                let baseline = destination.map_or(group.workspace_position[1], |_| origin_y);
                group.placed_overview_bounds(board).1[1] - baseline
            })
            .fold(panel_bottom, f32::max);
        [24.0, bottom + 48.0]
    }

    #[must_use]
    pub fn contains_workspace(&self, local_id: &str) -> bool {
        self.0.iter().any(|group| group.workspace == local_id)
    }

    pub fn reconcile(&mut self, board: &mut Board) {
        let mut registered = Vec::new();
        for group in &self.0 {
            if board
                .cloud_groups
                .0
                .iter()
                .any(|stored| stored.environment.id == group.environment.id)
            {
                continue;
            }
            if let Some(workspace) = board.workspace_id_by_local_id(&group.workspace)
                && !registered.iter().any(|(id, _)| *id == workspace)
            {
                registered.push((workspace, board.workspace_frame_rect(workspace)));
            }
            board.cloud_groups.0.push(group.clone());
        }
        // A preset applied to the board alone placed slot clouds there; this list must not
        // write its older geometry back over them.
        self.adopt_slot_geometry(board);
        // Register every member before any preset can mistake it for a free panel.
        for index in 0..self.0.len() {
            let before = self.0[index].size;
            self.0[index].reconcile(board);
            if self.0[index].size.iter().zip(before).any(|(new, old)| *new > old) {
                self.make_room(board, index);
            }
        }
        for (workspace, before) in registered {
            board.reapply_workspace_layout_after(workspace, before);
        }
        self.adopt_slot_geometry(board);
    }

    /// Resize within a cloud without invoking the parent workspace's collision policy.
    pub fn resize_panel(&mut self, board: &mut Board, id: PanelId, size: [f32; 2]) -> bool {
        let Some(panel) = board.panel(id) else { return false };
        let Some(index) = self.0.iter().position(|g| g.panels.contains(&panel.local_id)) else {
            return false;
        };
        let group = &mut self.0[index];
        if group.layout.is_some() {
            for member in &mut board.panels {
                if group.panels.contains(&member.local_id) && member.visible {
                    member.resize_layout(size);
                }
            }
            group.arrange(board);
        } else if let Some(panel) = board.panel_mut(id) {
            panel.resize_layout(size);
            for (axis, extent) in size.iter().enumerate() {
                group.size[axis] =
                    group.size[axis].max(panel.layout.position[axis] - group.position[axis] + extent + PAD);
            }
        }
        // In a workspace preset the cloud's new frame is the size every slot takes.
        if board.cloud_takes_slot(group) {
            let (environment, frame) = (group.environment.id.clone(), group.size);
            board.place_cloud_slot(&environment, frame);
            self.adopt_slot_geometry(board);
            return true;
        }
        group.reconcile_with_collisions(board, false);
        self.make_room_with_collisions(board, index, false);
        true
    }

    /// Make room for an expanded frame without changing any panel's membership.
    pub fn make_room(&mut self, board: &mut Board, expanded: usize) {
        self.make_room_with_collisions(board, expanded, true);
    }

    fn make_room_with_collisions(&mut self, board: &mut Board, expanded: usize, resolve_collisions: bool) {
        let order: Vec<_> = std::iter::once(expanded)
            .chain((0..self.0.len()).filter(|i| *i != expanded))
            .collect();
        for (offset, &index) in order.iter().enumerate().skip(1) {
            for _ in 0..offset {
                for &other in &order[..offset] {
                    if self.0[index].workspace != self.0[other].workspace {
                        continue;
                    }
                    let (min, max) = self.0[index].overview_bounds();
                    let (other_min, other_max) = self.0[other].overview_bounds();
                    if min[0] < other_max[0] && max[0] > other_min[0] && min[1] < other_max[1] && max[1] > other_min[1]
                    {
                        self.0[index].translate_with_collisions(
                            board,
                            [other_max[0] + PAD * 2.0 - min[0], 0.0],
                            resolve_collisions,
                        );
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn contains_panel(&self, board: &Board, id: PanelId) -> bool {
        board
            .panel(id)
            .is_some_and(|p| self.0.iter().any(|g| g.panels.contains(&p.local_id)))
    }

    #[must_use]
    pub fn at_position(&self, board: &Board, workspace: WorkspaceId, position: [f32; 2]) -> Option<usize> {
        self.0.iter().position(|g| {
            let (min, max) = g.bounds();
            !g.collapsed
                && board.workspace_id_by_local_id(&g.workspace) == Some(workspace)
                && position[0] >= min[0]
                && position[0] <= max[0]
                && position[1] >= min[1]
                && position[1] <= max[1]
        })
    }

    pub fn adopt_intersecting(&mut self, board: &Board) {
        for panel in &board.panels {
            if !panel.visible || self.0.iter().any(|g| g.panels.contains(&panel.local_id)) {
                continue;
            }
            let p = panel.layout.position;
            let size = panel.layout.size;
            let winner = self
                .0
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.remote.is_none()
                        && !g.collapsed
                        && board.workspace_id_by_local_id(&g.workspace) == Some(panel.workspace_id)
                })
                .map(|(index, g)| {
                    let (min, max) = g.bounds();
                    let width = (max[0].min(p[0] + size[0]) - min[0].max(p[0])).max(0.0);
                    let height = (max[1].min(p[1] + size[1]) - (min[1] + HEADER).max(p[1])).max(0.0);
                    (index, width * height)
                })
                .filter(|(_, area)| *area > 0.0)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index);
            if let Some(index) = winner {
                self.0[index].panels.push(panel.local_id.clone());
            }
        }
    }

    #[must_use]
    pub fn fitted_view(min: [f32; 2], max: [f32; 2], canvas: [f32; 2]) -> crate::CanvasViewState {
        let zoom = crate::clamp_canvas_zoom(
            ((canvas[0] - 70.0) / (max[0] - min[0]).max(1.0)).min((canvas[1] - 140.0) / (max[1] - min[1]).max(1.0)),
        )
        .min(1.0);
        crate::CanvasViewState::new(
            [
                canvas[0] * 0.5 - min[0].midpoint(max[0]) * zoom,
                35.0 + canvas[1] * 0.5 - min[1].midpoint(max[1]) * zoom,
            ],
            zoom,
        )
    }

    /// Fit an unobstructed canvas region using the shared persisted zoom limits.
    #[must_use]
    pub fn fitted_region(min: [f32; 2], max: [f32; 2], origin: [f32; 2], size: [f32; 2]) -> crate::CanvasViewState {
        let zoom = crate::clamp_canvas_zoom(
            ((size[0] - 24.0).max(1.0) / (max[0] - min[0]).max(1.0))
                .min((size[1] - 24.0).max(1.0) / (max[1] - min[1]).max(1.0)),
        )
        .min(1.0);
        crate::CanvasViewState::new(
            [
                origin[0] + size[0] * 0.5 - min[0].midpoint(max[0]) * zoom,
                origin[1] + size[1] * 0.5 - min[1].midpoint(max[1]) * zoom,
            ],
            zoom,
        )
    }

    pub fn restore_visibility(&mut self, board: &mut Board) {
        for group in &mut self.0 {
            if !group.collapsed {
                continue;
            }
            for panel in &mut board.panels {
                if group.panels.contains(&panel.local_id) {
                    panel.visible = false;
                    if board.focused == Some(panel.id) {
                        board.focused = None;
                    }
                }
            }
        }
    }

    /// Extent of a workspace's clouds and their runtime cards, where they are drawn.
    #[must_use]
    pub(crate) fn workspace_extent(&self, board: &Board, workspace: WorkspaceId) -> Option<([f32; 2], [f32; 2])> {
        let local_id = &board.workspace(workspace)?.local_id;
        self.0
            .iter()
            .filter(|group| &group.workspace == local_id)
            .map(|group| group.placed_overview_bounds(board))
            .reduce(crate::layout::union_bounds)
    }

    pub fn extend_workspace_bounds(&self, board: &Board, bounds: &mut HashMap<WorkspaceId, ([f32; 2], [f32; 2])>) {
        for group in &self.0 {
            let Some(id) = board.workspace_id_by_local_id(&group.workspace) else {
                continue;
            };
            let placed = group.placed_overview_bounds(board);
            bounds
                .entry(id)
                .and_modify(|entry| *entry = crate::layout::union_bounds(*entry, placed))
                .or_insert(placed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelKind, PanelOptions};

    #[test]
    fn a_production_cloud_keeps_its_panels_a_gap_below_the_painted_header() {
        let mut group = CloudGroup::new(101, "test".into(), "workspace".into(), ".".into(), [0.0, 0.0]);
        assert!((group.header_height() - group.header_chrome_height()).abs() < f32::EPSILON);
        let legacy = serde_json::json!({
            "deployment_started": true, "id": "cloud", "revision": "a".repeat(40), "profile_name": "dev",
            "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8},
        });
        group.remote = Some(serde_json::from_value(legacy).unwrap());
        assert!((group.header_height() - group.header_chrome_height() - CONTENT_GAP).abs() < f32::EPSILON);
        assert!(group.header_height() > group.header_chrome_height());
    }

    #[test]
    fn collapsing_a_cloud_whose_panel_is_hidden_for_disposal_saves_it_expandable() {
        let mut board = Board::new();
        let workspace = board.create_workspace("cloud");
        let panel = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Editor,
                    ..PanelOptions::default()
                },
                workspace,
            )
            .unwrap();
        let local = board.workspace(workspace).unwrap().local_id.clone();
        let mut group = CloudGroup::new(101, "cloud".into(), local, ".".into(), [0.0, 0.0]);
        group.attach(&mut board, panel);
        assert!(board.hide_for_disposal(panel));

        group.set_collapsed(&mut board, true);
        let member = board.panel(panel).unwrap().local_id.clone();
        assert!(
            group.hidden.contains(&member),
            "the collapse owns it now, so it is saved and can expand"
        );
        assert!(!board.is_hidden_for_disposal(panel), "the disposal marker has ended");
        group.set_collapsed(&mut board, false);
        assert!(board.panel(panel).unwrap().visible, "expanding shows it");
    }

    #[test]
    fn a_closing_cloud_gives_its_body_to_the_disposal_even_with_panels() {
        let mut group = CloudGroup::new(101, "test".into(), "workspace".into(), ".".into(), [0.0, 0.0]);
        let legacy = serde_json::json!({
            "deployment_started": true, "id": "cloud", "revision": "a".repeat(40), "profile_name": "dev",
            "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8},
        });
        group.remote = Some(serde_json::from_value(legacy).unwrap());
        group.panels.push("member".into());
        let (min, max) = group.runtime_bounds();
        assert!((max[1] - min[1]).abs() < f32::EPSILON, "its panels own the body");
        let (min, max) = group.runtime_bounds_while(true);
        assert!(max[1] - min[1] > 1.0, "its disposal owns the body while it is closed");
        group.collapsed = true;
        let (min, max) = group.runtime_bounds_while(true);
        assert!((max[1] - min[1]).abs() < f32::EPSILON, "a collapsed cloud has no body");
    }

    #[test]
    fn a_cloud_keeps_its_placement_and_records_without_one_keep_their_encoding() {
        let mut settings = vec!["EU-RO-1".to_owned(), "US-MO-2".to_owned()];
        let mut gpus = vec!["NVIDIA RTX A6000".to_owned()];
        Placement::default().apply(&mut settings, &mut gpus);
        assert_eq!(
            settings,
            ["EU-RO-1", "US-MO-2"],
            "any placement keeps the machine setting"
        );
        let europe = Placement {
            cpu_types: Vec::new(),
            region: Some("Europe".into()),
            data_centers: vec!["EU-RO-1".into(), "EUR-IS-1".into()],
            gpu_types: Vec::new(),
        };
        europe.apply(&mut settings, &mut gpus);
        assert_eq!(settings, ["EU-RO-1", "EUR-IS-1"]);
        assert_eq!(
            gpus,
            ["NVIDIA RTX A6000"],
            "no GPU choice keeps the machine preferences"
        );
        let a5000 = Placement {
            gpu_types: vec!["NVIDIA RTX A5000".into()],
            ..Placement::default()
        };
        a5000.apply(&mut settings, &mut gpus);
        assert_eq!(gpus, ["NVIDIA RTX A5000"]);
        assert_eq!(
            settings,
            ["EU-RO-1", "EUR-IS-1"],
            "a GPU choice alone keeps the data centers"
        );
        assert!(a5000.is_any() && !a5000.is_default());
        assert_eq!(a5000.for_profile(true), a5000);
        assert!(a5000.for_profile(false).is_default(), "a CPU cloud keeps no GPU choice");
        assert_eq!(europe.for_profile(false), europe);

        let legacy = serde_json::json!({
            "deployment_started": true, "id": "cloud", "revision": "a".repeat(40), "profile_name": "dev",
            "profile": {"provider": "runpod", "image": "example.invalid/worker", "cpu": 4, "memory_gb": 8},
        });
        let mut launch: CloudLaunch = serde_json::from_value(legacy).unwrap();
        assert!(launch.placement.is_any());
        assert!(serde_json::to_value(&launch).unwrap().get("placement").is_none());
        launch.placement = europe.clone();
        let saved = serde_json::to_value(&launch).unwrap();
        assert_eq!(saved["placement"]["region"], "Europe");
        assert!(saved["placement"].get("gpu_types").is_none());
        assert_eq!(serde_json::from_value::<CloudLaunch>(saved).unwrap().placement, europe);
        launch.placement = a5000.clone();
        let saved = serde_json::to_value(&launch).unwrap();
        assert_eq!(saved["placement"]["gpu_types"][0], "NVIDIA RTX A5000");
        assert_eq!(serde_json::from_value::<CloudLaunch>(saved).unwrap().placement, a5000);
    }

    #[test]
    fn chosen_siblings_persist_with_their_cloud_and_are_omitted_when_none() {
        let mut group = CloudGroup::new(101, "Siblings".into(), "desk".into(), "/synthetic/app".into(), [0.0; 2]);
        let saved = serde_json::to_value(&group).unwrap();
        assert!(saved.get("siblings").is_none(), "earlier records keep their encoding");
        assert!(serde_json::from_value::<CloudGroup>(saved).unwrap().siblings.is_empty());
        group.siblings = vec![crate::cloud_runtime::siblings::Binding {
            alias: "consumer".into(),
            local_repository: "/synthetic/consumer".into(),
            revision: Some("c".repeat(40)),
        }];
        let saved = serde_json::to_value(&group).unwrap();
        assert_eq!(saved["siblings"][0]["local_repository"], "/synthetic/consumer");
        // The reviewed commit persists, so a retry deploys exactly what was checked.
        assert_eq!(saved["siblings"][0]["revision"], "c".repeat(40));
        assert_eq!(
            serde_json::from_value::<CloudGroup>(saved).unwrap().siblings,
            group.siblings
        );
    }

    #[test]
    fn fractional_panel_resize_keeps_containment_bounds_ordered() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![CloudGroup::new(
            1,
            "a".into(),
            local,
            PathBuf::new(),
            [24.0, 798.0],
        )]);
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    size: Some(CHILD_SIZE),
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        groups.0[0].attach(&mut board, id);
        groups.0[0].set_layout(&mut board, Some(WorkspaceLayout::Grid));
        for delta in 0..100_u16 {
            assert!(groups.resize_panel(
                &mut board,
                id,
                [520.0 + f32::from(delta) * 0.1, 500.0 + f32::from(delta) * 0.1]
            ));
            groups.reconcile(&mut board);
            assert!(board.panel(id).unwrap().layout.position.iter().all(|p| p.is_finite()));
        }
    }

    #[test]
    fn persisted_membership_is_repaired_without_allowing_user_moves() {
        let mut board = Board::new();
        let home = board.create_workspace("cloud home");
        let other = board.create_workspace("other");
        let panel = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..Default::default()
                },
                home,
            )
            .unwrap();
        let local_id = board.panel(panel).unwrap().local_id.clone();
        let home_local = board.workspace(home).unwrap().local_id.clone();
        let mut group = CloudGroup::new(1, "cloud".into(), home_local.clone(), PathBuf::new(), [0.0, 0.0]);
        group.attach(&mut board, panel);
        board.cloud_groups = CloudGroups(vec![group]);
        let mut saved = crate::RuntimeState::from_board(
            &board,
            crate::WindowConfig::default(),
            crate::CanvasViewState::default(),
        );
        let member = saved.workspaces[0].panels.remove(0);
        saved.workspaces[1].panels.push(member);
        let mut restored = Board::from_runtime_state(&saved).unwrap();
        let panel = restored.panels.iter().find(|p| p.local_id == local_id).unwrap().id;
        let home = restored.workspace_id_by_local_id(&home_local).unwrap();
        assert_ne!(restored.panel_workspace_id(panel), Some(home));
        let mut groups = restored.cloud_groups.clone();
        groups.reconcile(&mut restored);
        assert_eq!(restored.panel_workspace_id(panel), Some(home));
        assert!(restored.workspace(home).unwrap().panels.contains(&panel));
        assert!(!restored.workspace(other).unwrap().panels.contains(&panel));
        restored.assign_panel_to_workspace(panel, other);
        assert_eq!(restored.panel_workspace_id(panel), Some(home));
    }

    #[test]
    fn resizing_later_child_resizes_grid_and_moves_neighbor_with_its_runtime() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![
            CloudGroup::new(1, "a".into(), local.clone(), PathBuf::new(), [0.0, 0.0]),
            CloudGroup::new(2, "b".into(), local, PathBuf::new(), [1450.0, 0.0]),
        ]);
        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        size: Some(CHILD_SIZE),
                        ..PanelOptions::default()
                    },
                    ws,
                )
                .unwrap();
            groups.0[0].attach(&mut board, id);
            ids.push(id);
        }
        groups.0[0].set_layout(&mut board, Some(WorkspaceLayout::Grid));
        assert!(groups.resize_panel(&mut board, ids[1], [800.0, 600.0]));
        groups.reconcile(&mut board);
        for id in &ids {
            assert!(
                board
                    .panel(*id)
                    .unwrap()
                    .layout
                    .size
                    .into_iter()
                    .zip([800.0, 600.0])
                    .all(|(a, b)| (a - b).abs() < f32::EPSILON)
            );
        }
        assert!(groups.0[1].position[0] > groups.0[0].overview_bounds().1[0]);
        board.panel_mut(ids[0]).unwrap().resize_layout([1000.0, 600.0]);
        groups.reconcile(&mut board);
        assert!(groups.0[1].position[0] > groups.0[0].overview_bounds().1[0]);
        board.close_panel(ids[1]);
        groups.reconcile(&mut board);
        assert_eq!(groups.0[0].panels.len(), 1);
        assert!(groups.0[1].panels.is_empty());
    }

    #[test]
    fn independent_layouts_reflow_members_and_survive_serialization() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut a = CloudGroup::new(1, "a".into(), local.clone(), PathBuf::new(), [0.0, 0.0]);
        let mut b = CloudGroup::new(2, "b".into(), local, PathBuf::new(), [1800.0, 0.0]);
        for group in [&mut a, &mut b] {
            for _ in 0..3 {
                let id = board
                    .create_panel(
                        PanelOptions {
                            kind: PanelKind::Usage,
                            size: Some(CHILD_SIZE),
                            ..PanelOptions::default()
                        },
                        ws,
                    )
                    .unwrap();
                group.attach(&mut board, id);
            }
        }
        a.set_layout(&mut board, Some(WorkspaceLayout::Rows));
        b.set_layout(&mut board, Some(WorkspaceLayout::Grid));
        let positions = |group: &CloudGroup, board: &Board| -> Vec<[f32; 2]> {
            group
                .panels
                .iter()
                .map(|id| board.panels.iter().find(|p| &p.local_id == id).unwrap().layout.position)
                .collect()
        };
        let before_b = positions(&b, &board);
        let rows = positions(&a, &board);
        assert!((rows[0][0] - rows[1][0]).abs() < f32::EPSILON);
        assert!(rows[0][1] < rows[1][1]);
        assert!((before_b[0][1] - before_b[1][1]).abs() < f32::EPSILON);
        assert!(before_b[2][1] > before_b[0][1]);
        a.set_layout(&mut board, Some(WorkspaceLayout::Columns));
        assert_eq!(positions(&b, &board), before_b);
        let columns = positions(&a, &board);
        assert!((columns[0][1] - columns[1][1]).abs() < f32::EPSILON);
        assert!(columns[0][0] < columns[1][0]);
        let encoded = serde_json::to_string(&CloudGroups(vec![a, b])).unwrap();
        let restored: CloudGroups = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.0[0].layout, Some(WorkspaceLayout::Columns));
        assert_eq!(restored.0[1].layout, Some(WorkspaceLayout::Grid));
        assert_eq!(restored.0[0].panels.len(), 3);
        assert_eq!(restored.0[1].panels.len(), 3);
    }

    fn attach_usage_panels(board: &mut Board, group: &mut CloudGroup, positions: &[[f32; 2]]) -> Vec<PanelId> {
        let workspace = board.workspace_id_by_local_id(&group.workspace).unwrap();
        positions
            .iter()
            .map(|position| {
                let id = board
                    .create_panel(
                        PanelOptions {
                            kind: PanelKind::Usage,
                            position: Some(*position),
                            size: Some(CHILD_SIZE),
                            ..PanelOptions::default()
                        },
                        workspace,
                    )
                    .unwrap();
                group.attach(board, id);
                id
            })
            .collect()
    }

    #[test]
    fn new_cloud_starts_with_grid_and_arranges_members_added_later() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        board.workspace_mut(ws).unwrap().layout = None;
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![CloudGroup::new(
            1,
            "Cloud".into(),
            local,
            PathBuf::new(),
            [0.0, 0.0],
        )]);
        assert_eq!(groups.0[0].layout, Some(WorkspaceLayout::Grid));
        groups.reconcile(&mut board);
        let ids = attach_usage_panels(
            &mut board,
            &mut groups.0[0],
            &[[900.0, 700.0], [60.0, 1500.0], [400.0, 90.0]],
        );
        let placed: Vec<_> = ids.iter().map(|id| board.panel(*id).unwrap().layout.position).collect();
        // Three members fill two columns; the third starts a row below the first.
        assert_eq!(placed[1][1].to_bits(), placed[0][1].to_bits());
        assert!(placed[1][0] >= placed[0][0] + CHILD_SIZE[0]);
        assert_eq!(placed[2][0].to_bits(), placed[0][0].to_bits());
        assert!(placed[2][1] >= placed[0][1] + CHILD_SIZE[1]);
        let (min, max) = groups.0[0].bounds();
        for position in placed {
            assert!(position[0] >= min[0] + PAD && position[0] + CHILD_SIZE[0] <= max[0]);
            assert!(position[1] >= min[1] + HEADER && position[1] + CHILD_SIZE[1] <= max[1]);
        }
    }

    #[test]
    fn saved_cloud_without_a_stored_layout_keeps_manual_placement() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("desk", [0.0, 0.0]);
        board.workspace_mut(ws).unwrap().layout = None;
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![CloudGroup::new(
            1,
            "Cloud".into(),
            local,
            PathBuf::new(),
            [0.0, 0.0],
        )]);
        groups.0[0].layout = None;
        groups.reconcile(&mut board);
        let ids = attach_usage_panels(
            &mut board,
            &mut groups.0[0],
            &[[PAD, HEADER], [PAD + 600.0, HEADER + 40.0]],
        );
        let positions = |board: &Board| -> Vec<[u32; 2]> {
            ids.iter()
                .map(|id| board.panel(*id).unwrap().layout.position.map(f32::to_bits))
                .collect()
        };
        let manual = positions(&board);
        assert_ne!(manual[0][1], manual[1][1], "a grid would align both members in one row");
        let saved = serde_json::to_value(&groups).unwrap();
        for stored in [None, Some(serde_json::Value::Null)] {
            let mut value = saved.clone();
            let group = value[0].as_object_mut().unwrap();
            group.remove("layout");
            if let Some(stored) = stored {
                group.insert("layout".into(), stored);
            }
            let mut restored: CloudGroups = serde_json::from_value(value).unwrap();
            assert_eq!(restored.0[0].layout, None);
            restored.reconcile(&mut board);
            assert_eq!(positions(&board), manual);
        }
    }

    #[test]
    fn collapse_move_expand_preserves_panel_identity_and_prior_visibility() {
        let mut board = Board::new();
        let ws = board.create_workspace("test");
        let mut group = CloudGroup::new(
            1,
            "issue".into(),
            board.workspace(ws).unwrap().local_id.clone(),
            PathBuf::new(),
            [0.0, 0.0],
        );
        let a = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        let b = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        group.attach(&mut board, a);
        group.attach(&mut board, b);
        board.panel_mut(b).unwrap().visible = false;
        let before = board.panel(a).unwrap().layout.position;
        group.set_collapsed(&mut board, true);
        assert!(!board.panel(a).unwrap().visible);
        group.translate(&mut board, [10.0, 20.0]);
        group.set_collapsed(&mut board, false);
        assert!(board.panel(a).unwrap().visible);
        assert!(!board.panel(b).unwrap().visible);
        let after = board.panel(a).unwrap().layout.position;
        assert!((after[0] - before[0] - 10.0).abs() < f32::EPSILON);
        assert!((after[1] - before[1] - 20.0).abs() < f32::EPSILON);
        assert_eq!(board.panels.len(), 2);
        board.close_panel(a);
        group.reconcile(&mut board);
        assert_eq!(group.panels.len(), 1);
    }
    #[test]
    fn binding_survives_drag_into_another_cloud_and_workspace() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("test", [0.0, 0.0]);
        let other = board.create_workspace("other");
        let local = board.workspace(ws).unwrap().local_id.clone();
        let mut groups = CloudGroups(vec![
            CloudGroup::new(1, "a".into(), local.clone(), PathBuf::from("a"), [0.0, 0.0]),
            CloudGroup::new(2, "b".into(), local, PathBuf::from("b"), [1000.0, 0.0]),
        ]);
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    position: Some([20.0, HEADER]),
                    size: Some(CHILD_SIZE),
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        groups.adopt_intersecting(&board);
        assert_eq!(groups.0[0].panels.len(), 1);
        board.panel_mut(id).unwrap().layout.position = [1100.0, HEADER];
        groups.adopt_intersecting(&board);
        for group in &mut groups.0 {
            group.reconcile(&mut board);
        }
        assert!(groups.0[1].panels.is_empty());
        assert!(board.panel(id).unwrap().layout.position[0] < 1000.0);
        board.assign_panel_to_workspace(id, other);
        groups.0[0].reconcile(&mut board);
        assert_eq!(board.panel(id).unwrap().workspace_id, ws);
    }

    #[test]
    fn empty_cloud_keeps_its_workspace_after_restore_and_cannot_move_members() {
        let mut board = Board::new();
        let ws = board.create_workspace("cloud workspace");
        let other = board.create_workspace("other workspace");
        let local = board.workspace(ws).unwrap().local_id.clone();
        board.cloud_groups.0.push(CloudGroup::new(
            1,
            "Cloud".into(),
            local.clone(),
            PathBuf::new(),
            [0.0, 0.0],
        ));
        let snapshot = crate::RuntimeState::from_board(
            &board,
            crate::WindowConfig::default(),
            crate::CanvasViewState::default(),
        );
        let mut restored = Board::from_runtime_state(&snapshot).unwrap();
        restored.remove_empty_workspaces();
        let restored_ws = restored.workspace_id_by_local_id(&local).unwrap();
        restored.remove_workspace(restored_ws);
        assert!(restored.workspace(restored_ws).is_some());
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    ..PanelOptions::default()
                },
                ws,
            )
            .unwrap();
        let mut group = board.cloud_groups.0[0].clone();
        group.attach(&mut board, id);
        board.cloud_groups.0[0] = group;
        board.assign_panel_to_workspace(id, other);
        assert_eq!(board.panel_workspace_id(id), Some(ws));
        let mut second = CloudGroup::new(2, "Other cloud".into(), local, PathBuf::new(), [800.0, 0.0]);
        second.attach(&mut board, id);
        assert!(second.panels.is_empty());
    }

    #[test]
    fn collapsed_groups_restore_hidden_and_last_child_keeps_workspace() {
        let mut board = Board::new();
        let ws = board.create_workspace_at("test", [0.0, 0.0]);
        let mut groups = CloudGroups::default();
        for issue in [1, 2] {
            let id = board
                .create_panel(
                    PanelOptions {
                        kind: PanelKind::Usage,
                        ..PanelOptions::default()
                    },
                    ws,
                )
                .unwrap();
            let mut group = CloudGroup::new(
                issue,
                "issue".into(),
                board.workspace(ws).unwrap().local_id.clone(),
                PathBuf::new(),
                [0.0, 0.0],
            );
            group.attach(&mut board, id);
            group.set_collapsed(&mut board, true);
            groups.0.push(group);
        }
        let snapshot = crate::RuntimeState::from_board(
            &board,
            crate::WindowConfig::default(),
            crate::CanvasViewState::default(),
        );
        let mut restored = Board::from_runtime_state(&snapshot).unwrap();
        groups.restore_visibility(&mut restored);
        for group in &mut groups.0 {
            group.reconcile(&mut restored);
        }
        assert!(restored.panels.iter().all(|p| !p.visible));
        assert!(groups.0.iter().all(|g| g.collapsed));
        restored.retain_workspace_when_empty(ws);
        let ids: Vec<_> = restored.panels.iter().map(|p| p.id).collect();
        for id in ids {
            restored.close_panel(id);
        }
        assert!(restored.workspace(ws).is_some());
    }
    #[test]
    fn new_cloud_position_clears_existing_local_panels() {
        let mut board = Board::new();
        let workspace = board.create_workspace_at("Local", [500.0, 200.0]);
        let id = board
            .create_panel(
                PanelOptions {
                    kind: PanelKind::Usage,
                    position: Some([550.0, 300.0]),
                    size: Some([600.0, 400.0]),
                    ..PanelOptions::default()
                },
                workspace,
            )
            .unwrap();
        let panel = board.panel(id).unwrap();
        let occupied = crate::board::panel_visual_rect(panel.layout.position, panel.layout.size);
        let position_before = panel.layout.position;
        let size_before = panel.layout.size;
        let local = board.workspace(workspace).unwrap().local_id.clone();
        let groups = CloudGroups::default();
        let position = groups.next_position(&local, &board);
        let mut group = CloudGroup::new(1, "Cloud".into(), local, PathBuf::new(), position);
        group.reconcile(&mut board);
        assert!(group.position[1] >= occupied[3] + 48.0);
        let panel = board.panel(id).unwrap();
        assert!(
            panel
                .layout
                .position
                .into_iter()
                .zip(position_before)
                .all(|(left, right)| (left - right).abs() <= f32::EPSILON)
        );
        assert!(
            panel
                .layout
                .size
                .into_iter()
                .zip(size_before)
                .all(|(left, right)| (left - right).abs() <= f32::EPSILON)
        );
    }
}
