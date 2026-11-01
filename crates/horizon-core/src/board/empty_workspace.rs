//! Removing a workspace that has nothing in it.
use super::Board;
use crate::{runtime_state::cloud_groups, workspace::WorkspaceId};

impl Board {
    /// Whether [`Board::remove_empty_workspace`] would remove `id`: it holds no panel, no
    /// cloud and no remote workspace, and the board keeps another workspace. A menu asks
    /// this before it offers the removal, so a person never gets a button that does nothing.
    #[must_use]
    pub fn can_remove_empty_workspace(&self, id: WorkspaceId) -> bool {
        let Some(workspace) = self.workspace(id) else {
            return false;
        };
        // A remote workspace's reference is state of its own, as the other cleanups keep it.
        self.workspaces.len() > 1
            && workspace.remote_workspace.is_none()
            && workspace.panels.is_empty()
            && !self.panels.iter().any(|panel| panel.workspace_id == id)
            && !cloud_groups::contains_workspace(&self.cloud_groups, &workspace.local_id)
    }

    /// Removes `id` when it has no panel, no cloud and no remote workspace, and returns
    /// whether it did. Unlike
    /// [`Board::remove_workspace`] it needs no other ordinary workspace to take its panels,
    /// so it also works on a board of clouds; the board still keeps one workspace.
    pub fn remove_empty_workspace(&mut self, id: WorkspaceId) -> bool {
        if !self.can_remove_empty_workspace(id) {
            return false;
        }
        let Some(index) = self.workspaces.iter().position(|workspace| workspace.id == id) else {
            return false;
        };
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
    fn the_predicate_answers_what_the_removal_does() {
        let mut board = Board::new();
        let only = board.create_workspace("only");
        assert!(!board.can_remove_empty_workspace(only), "the board keeps one workspace");
        let empty = board.create_workspace("empty");
        assert!(board.can_remove_empty_workspace(empty));
        assert!(board.remove_empty_workspace(empty));
        assert!(
            !board.can_remove_empty_workspace(empty),
            "a gone workspace is not removable"
        );
    }

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
        // A remote workspace without local panels keeps its reference.
        let remote = board.create_workspace("remote");
        board.workspace_mut(remote).unwrap().remote_workspace = Some(
            crate::runtime_state::RemoteWorkspaceReference::new(
                "11111111-1111-4111-8111-111111111111".into(),
                "remote-environment".into(),
            )
            .unwrap(),
        );
        assert!(!board.remove_empty_workspace(remote));
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
