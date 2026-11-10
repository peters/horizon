//! Removing a workspace that has nothing in it.
use super::Board;
use crate::workspace::WorkspaceId;

impl Board {
    /// Whether [`Board::remove_empty_workspace`] would remove `id`: it holds no panel,
    /// and the board keeps another workspace. A menu asks this before it offers the
    /// removal, so a person never gets a button that does nothing.
    #[must_use]
    pub fn can_remove_empty_workspace(&self, id: WorkspaceId) -> bool {
        // The board keeps one workspace, so the only one stays even when it holds nothing.
        self.workspaces.len() > 1 && self.workspace_holds_nothing(id)
    }

    /// Whether `id` holds nothing at all: no panel.
    #[must_use]
    pub fn workspace_holds_nothing(&self, id: WorkspaceId) -> bool {
        let Some(workspace) = self.workspace(id) else {
            return false;
        };
        workspace.panels.is_empty() && !self.panels.iter().any(|panel| panel.workspace_id == id)
    }

    /// Removes `id` when it has no panel, and returns whether it did. Unlike
    /// [`Board::remove_workspace`] it needs no other workspace to take its panels.
    /// The board still keeps one workspace.
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
    fn an_empty_workspace_goes_but_the_last_one_stays() {
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
}

#[cfg(test)]
mod reuse_tests {
    use super::*;

    #[test]
    fn a_workspace_that_holds_nothing_can_be_filled_but_the_remove_rule_keeps_the_last_one() {
        let mut board = Board::new();
        let only = board.create_workspace("only");
        assert!(board.workspace_holds_nothing(only), "an empty workspace holds nothing");
        assert!(!board.can_remove_empty_workspace(only), "the board keeps one workspace");
        let second = board.create_workspace("second");
        assert!(board.workspace_holds_nothing(second));
        assert!(board.can_remove_empty_workspace(second));
    }
}
