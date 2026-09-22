//! Reorder only visible siblings within their owning cloud's layout.
use super::{CloudGroup, CloudGroups};
use crate::{Board, PanelId, layout::point_in_panel};

impl CloudGroups {
    #[must_use]
    pub fn group_for_panel(&self, board: &Board, id: PanelId) -> Option<&CloudGroup> {
        let panel = board.panel(id)?;
        self.0.iter().find(|group| group.panels.contains(&panel.local_id))
    }

    /// Move a visible arranged panel into the sibling slot containing its dragged center.
    /// Membership, the parent workspace order and other clouds remain unchanged.
    pub fn reorder_panel_at(&mut self, board: &mut Board, id: PanelId, position: [f32; 2]) -> bool {
        let Some(panel) = board.panel(id) else { return false };
        if !panel.visible || !position.iter().all(|value| value.is_finite()) {
            return false;
        }
        let Some(group) = self.0.iter_mut().find(|group| group.panels.contains(&panel.local_id)) else {
            return false;
        };
        if group.layout.is_none()
            || group.collapsed
            || board.workspace_id_by_local_id(&group.workspace) != Some(panel.workspace_id)
        {
            return false;
        }
        let Some(source) = group.panels.iter().position(|local| local == &panel.local_id) else {
            return false;
        };
        let center = [
            position[0] + panel.layout.size[0] * 0.5,
            position[1] + panel.layout.size[1] * 0.5,
        ];
        if !center.iter().all(|value| value.is_finite()) {
            return false;
        }
        let target = group.panels.iter().position(|local| {
            board
                .panel_id_by_local_id(local)
                .and_then(|id| board.panel(id))
                .is_some_and(|candidate| {
                    candidate.id != id
                        && candidate.visible
                        && candidate.workspace_id == panel.workspace_id
                        && point_in_panel(center, candidate.layout.position, candidate.layout.size)
                })
        });
        let Some(target) = target else { return false };
        group.panels.swap(source, target);
        group.arrange(board);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelKind, PanelOptions, WorkspaceLayout};
    #[test]
    fn preset_reordering_is_scoped_and_survives_reconcile_and_serialization() {
        for layout in [WorkspaceLayout::Rows, WorkspaceLayout::Columns, WorkspaceLayout::Grid] {
            let mut board = Board::new();
            let workspace = board.create_workspace("fixture");
            let panels: Vec<_> = (0..5)
                .map(|_| {
                    board
                        .create_panel(
                            PanelOptions {
                                kind: PanelKind::Editor,
                                ..Default::default()
                            },
                            workspace,
                        )
                        .unwrap()
                })
                .collect();
            let local: Vec<_> = panels
                .iter()
                .map(|id| board.panel(*id).unwrap().local_id.clone())
                .collect();
            let mut group = CloudGroup::new(
                1,
                "Fixture".into(),
                board.workspace(workspace).unwrap().local_id.clone(),
                "/fixture".into(),
                [0.0, 0.0],
            );
            group.panels = local[..3].to_vec();
            board.panel_mut(panels[2]).unwrap().visible = false;
            group.set_layout(&mut board, Some(layout));
            let mut other = CloudGroup::new(
                2,
                "Other".into(),
                group.workspace.clone(),
                "/fixture".into(),
                [3000.0, 0.0],
            );
            other.panels.push(local[3].clone());
            let mut groups = CloudGroups(vec![group, other]);
            groups.reconcile(&mut board);
            let parent_order = board.workspace(workspace).unwrap().panels.clone();
            let first = board.panel(panels[0]).unwrap().layout.position;
            let target = board.panel(panels[1]).unwrap().layout.position;
            assert!(groups.reorder_panel_at(&mut board, panels[0], target));
            assert_eq!(
                groups.0[0].panels,
                [local[1].clone(), local[0].clone(), local[2].clone()]
            );
            assert_eq!(groups.0[1].panels, [local[3].clone()]);
            assert_eq!(board.workspace(workspace).unwrap().panels, parent_order);
            groups = serde_json::from_str(&serde_json::to_string(&groups).unwrap()).unwrap();
            groups.reconcile(&mut board);
            assert_eq!(groups.0[0].layout, Some(layout));
            assert_eq!(
                board.panel(panels[0]).unwrap().layout.position.map(f32::to_bits),
                target.map(f32::to_bits)
            );
            assert_eq!(
                board.panel(panels[1]).unwrap().layout.position.map(f32::to_bits),
                first.map(f32::to_bits)
            );
            for excluded in [panels[2], panels[3], panels[4]] {
                board.panel_mut(excluded).unwrap().move_to([8000.0, 8000.0]);
                assert!(!groups.reorder_panel_at(&mut board, panels[0], [8000.0, 8000.0]));
            }
            assert!(!groups.reorder_panel_at(&mut board, panels[2], target));
            assert!(!groups.reorder_panel_at(&mut board, panels[0], [f32::NAN, 0.0]));
            groups.0[0].layout = None;
            assert!(!groups.reorder_panel_at(&mut board, panels[0], first));
        }
    }
}
