//! Removing a workspace that has nothing in it.
use super::Board;
use crate::{runtime_state::cloud_groups, workspace::WorkspaceId};

impl Board {
    /// Removes `id` when it has no panel and no cloud, and returns whether it did. Unlike
    /// [`Board::remove_workspace`] it needs no other ordinary workspace to take its panels,
    /// so it also works on a board of clouds; the board still keeps one workspace.
    pub fn remove_empty_workspace(&mut self, id: WorkspaceId) -> bool {
        let Some(index) = self.workspaces.iter().position(|workspace| workspace.id == id) else {
            return false;
        };
        let workspace = &self.workspaces[index];
        if self.workspaces.len() <= 1
            || !workspace.panels.is_empty()
            || self.panels.iter().any(|panel| panel.workspace_id == id)
            || cloud_groups::contains_workspace(&self.cloud_groups, &workspace.local_id)
        {
            return false;
        }
        self.workspaces.remove(index);
        self.retained_empty_workspaces.remove(&id);
        if self.active_workspace == Some(id) {
            self.active_workspace = self.workspaces.first().map(|workspace| workspace.id);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_workspace_goes_beside_clouds_but_the_last_one_stays() {
        let mut board = Board::new();
        let only = board.create_workspace("only");
        assert!(!board.remove_empty_workspace(only), "the board keeps one workspace");
        let empty = board.create_workspace("empty");
        board.active_workspace = Some(empty);
        assert!(board.remove_empty_workspace(empty));
        assert!(board.workspace(empty).is_none());
        assert_eq!(board.active_workspace, Some(only));
        assert!(!board.remove_empty_workspace(empty), "already gone");
    }

    #[cfg(feature = "cloud-workspaces")]
    #[test]
    fn beside_only_clouds_an_empty_workspace_still_goes() {
        let mut board = Board::new();
        let cloud = board.create_workspace("cloud");
        let local = board.workspace(cloud).unwrap().local_id.clone();
        board.cloud_groups.0.push(crate::cloud_panel::CloudGroup::new(
            1,
            "Cloud".into(),
            local,
            "/synthetic".into(),
            [0.0, 0.0],
        ));
        let empty = board.create_workspace("empty");
        board.remove_workspace(empty);
        assert!(
            board.workspace(empty).is_some(),
            "remove_workspace needs an ordinary workspace"
        );
        assert!(board.remove_empty_workspace(empty));
        assert!(!board.remove_empty_workspace(cloud), "a cloud's workspace stays");
    }
}
