use crate::layout::TILE_GAP;
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

use super::super::Board;
use super::resize_collision_push;

/// Something a growing panel pushes aside. A frame moves as one unit with its
/// member panels, which never collide on their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Panel(PanelId),
    Frame(usize),
}

/// Cloud frames that share a workspace with ordinary panels.
trait CollisionFrames {
    fn frame_of(&self, board: &Board, panel: PanelId) -> Option<usize>;
    fn frames_in(&self, board: &Board, workspace: WorkspaceId) -> Vec<usize>;
    fn frame_rect(&self, frame: usize) -> Option<[f32; 4]>;
    fn translate_frame(&mut self, board: &mut Board, frame: usize, delta: [f32; 2]);
}

impl Board {
    /// After a panel is resized, push every overlapping sibling panel and
    /// cloud frame within the same workspace along the dominant
    /// resize-growth axis, cascading until nothing overlaps.
    pub(super) fn resolve_panel_collisions(
        &mut self,
        source: PanelId,
        workspace_id: WorkspaceId,
        resize_delta: [f32; 2],
    ) {
        #[cfg(feature = "cloud-workspaces")]
        let mut frames = std::mem::take(&mut self.cloud_groups);
        #[cfg(not(feature = "cloud-workspaces"))]
        let mut frames = NoFrames;
        self.resolve_body_collisions(&mut frames, source, workspace_id, resize_delta);
        #[cfg(feature = "cloud-workspaces")]
        {
            self.cloud_groups = frames;
        }
    }

    fn resolve_body_collisions(
        &mut self,
        frames: &mut impl CollisionFrames,
        source: PanelId,
        workspace_id: WorkspaceId,
        resize_delta: [f32; 2],
    ) {
        let bodies = self.collision_bodies(frames, workspace_id);
        let mut queue = vec![Body::Panel(source)];
        // A member growing inside its cloud must not push that cloud away.
        let mut settled: Vec<Body> = queue
            .iter()
            .copied()
            .chain(frames.frame_of(self, source).map(Body::Frame))
            .collect();

        while let Some(check) = queue.pop() {
            let Some(check_rect) = self.body_rect(frames, check) else {
                continue;
            };

            for &other in &bodies {
                if settled.contains(&other) {
                    continue;
                }
                let Some(other_rect) = self.body_rect(frames, other) else {
                    continue;
                };

                let push = resize_collision_push(check_rect, other_rect, resize_delta, TILE_GAP);
                if push[0] != 0.0 || push[1] != 0.0 {
                    match other {
                        Body::Panel(id) => {
                            if let Some(panel) = self.panel_mut(id) {
                                let position = panel.layout.position;
                                panel.move_to([position[0] + push[0], position[1] + push[1]]);
                            }
                        }
                        Body::Frame(frame) => frames.translate_frame(self, frame, push),
                    }
                    settled.push(other);
                    queue.push(other);
                }
            }
        }
    }

    fn collision_bodies(&self, frames: &impl CollisionFrames, workspace_id: WorkspaceId) -> Vec<Body> {
        let Some(workspace) = self.workspace(workspace_id) else {
            return Vec::new();
        };
        workspace
            .panels
            .iter()
            .copied()
            .filter(|id| frames.frame_of(self, *id).is_none())
            .map(Body::Panel)
            .chain(frames.frames_in(self, workspace_id).into_iter().map(Body::Frame))
            .collect()
    }

    fn body_rect(&self, frames: &impl CollisionFrames, body: Body) -> Option<[f32; 4]> {
        match body {
            Body::Panel(id) => self.panel(id).map(|panel| {
                let [x, y] = panel.layout.position;
                let [width, height] = panel.layout.size;
                [x, y, x + width, y + height]
            }),
            Body::Frame(frame) => frames.frame_rect(frame),
        }
    }
}

#[cfg(feature = "cloud-workspaces")]
impl CollisionFrames for crate::cloud_panel::CloudGroups {
    fn frame_of(&self, board: &Board, panel: PanelId) -> Option<usize> {
        let local_id = &board.panel(panel)?.local_id;
        self.0.iter().position(|group| group.panels.contains(local_id))
    }

    fn frames_in(&self, board: &Board, workspace: WorkspaceId) -> Vec<usize> {
        self.0
            .iter()
            .enumerate()
            .filter(|(_, group)| board.workspace_id_by_local_id(&group.workspace) == Some(workspace))
            .map(|(frame, _)| frame)
            .collect()
    }

    /// The runtime card beside a frame moves with it, so it collides too.
    fn frame_rect(&self, frame: usize) -> Option<[f32; 4]> {
        let (min, max) = self.0.get(frame)?.overview_bounds();
        Some([min[0], min[1], max[0], max[1]])
    }

    fn translate_frame(&mut self, board: &mut Board, frame: usize, delta: [f32; 2]) {
        if let Some(group) = self.0.get_mut(frame) {
            group.translate(board, delta);
        }
    }
}

#[cfg(not(feature = "cloud-workspaces"))]
struct NoFrames;

#[cfg(not(feature = "cloud-workspaces"))]
impl CollisionFrames for NoFrames {
    fn frame_of(&self, _board: &Board, _panel: PanelId) -> Option<usize> {
        None
    }

    fn frames_in(&self, _board: &Board, _workspace: WorkspaceId) -> Vec<usize> {
        Vec::new()
    }

    fn frame_rect(&self, _frame: usize) -> Option<[f32; 4]> {
        None
    }

    fn translate_frame(&mut self, _board: &mut Board, _frame: usize, _delta: [f32; 2]) {}
}
