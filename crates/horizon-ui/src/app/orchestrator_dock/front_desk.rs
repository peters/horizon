//! The front desk of the dock: the one conversation that belongs to no workspace,
//! and the choice of the conversation that a frame draws.
//!
//! A workspace conversation is keyed by the `local_id` of its workspace. The front
//! desk uses the reserved key [`FRONT_DESK`] instead. A `local_id` is a UUID, so
//! the two kinds of key cannot meet, and the pass of the dock that forgets the
//! conversations of workspaces that left the board never forgets this one.
//!
//! The dock shows one conversation at a time. The start of Horizon shows the front
//! desk; a press on a row of the workspace list and every change of the active
//! workspace show that workspace; the back control of the header and the command
//! `Show Front Desk` show the front desk again; and a board without an active
//! workspace shows the front desk, because no conversation of a workspace belongs
//! to it.
//!
//! The record of the front desk is the one that a restart resumes. It stands
//! beside the workspaces of the runtime state (`RuntimeState.front_desk`), because
//! a board holds no workspace for this conversation.

use horizon_core::{Board, OrchestratorConversationState, WorkspaceId};

/// The key of the conversation of the front desk. It is also the name of the
/// folder of the front desk under the Horizon home, and the name of its stored
/// file in the session folder.
pub(in crate::app) const FRONT_DESK: &str = "front-desk";

/// The front desk of this run, and the conversation that the dock shows.
pub(in crate::app) struct FrontDesk {
    /// The record of the front desk in the runtime state of this session, which a
    /// restart resumes.
    record: Option<OrchestratorConversationState>,
    /// Whether the dock shows the front desk instead of a workspace.
    shown: bool,
    /// Whether a frame read the active workspace of the board yet, and which one
    /// it read. A workspace that the start restored is no change of this run.
    active_read: bool,
    active_seen: Option<WorkspaceId>,
}

impl Default for FrontDesk {
    fn default() -> Self {
        Self {
            record: None,
            // The start of Horizon shows the front desk.
            shown: true,
            active_read: false,
            active_seen: None,
        }
    }
}

impl FrontDesk {
    /// The key of the conversation that the dock shows on `board`: [`FRONT_DESK`],
    /// or the `local_id` of the active workspace of that board.
    pub(in crate::app) fn key<'a>(&self, board: &'a Board) -> &'a str {
        let active = board
            .active_workspace
            .and_then(|id| board.workspace(id))
            .map(|workspace| workspace.local_id.as_str());
        match active {
            Some(local_id) if !self.shown => local_id,
            _ => FRONT_DESK,
        }
    }

    /// Whether the dock shows the front desk on `board`, which the sidebar reads to
    /// draw no active workspace row and the button `+ Rent a machine` reads to
    /// disable itself.
    pub(in crate::app) fn shows(&self, board: &Board) -> bool {
        self.key(board) == FRONT_DESK
    }

    /// Shows the front desk, which the back control of the header and the command
    /// `Show Front Desk` of the palette do.
    pub(in crate::app) fn show(&mut self) {
        self.shown = true;
    }

    /// Shows the conversation of the active workspace, which a press on a row of
    /// the sidebar does.
    pub(in crate::app) fn show_workspace(&mut self) {
        self.shown = false;
    }

    /// Follows the active workspace of `board`: a change of it shows that
    /// workspace. The dock of a run whose start restored a workspace keeps the
    /// front desk, because a restored workspace is no change.
    pub(in crate::app) fn follow_active_workspace(&mut self, board: &Board) {
        let active = board.active_workspace;
        if self.active_read && self.active_seen != active {
            self.shown = false;
        }
        self.active_read = true;
        self.active_seen = active;
    }

    /// The record of the front desk in the runtime state of this session, which
    /// the exit saves.
    pub(in crate::app) fn record(&self) -> Option<&OrchestratorConversationState> {
        self.record.as_ref()
    }

    /// Keeps the record of the front desk of the session that this start opens.
    pub(in crate::app) fn restore(&mut self, record: Option<OrchestratorConversationState>) {
        self.record = record;
    }

    /// Keeps the record that the conversation of the front desk names, which the
    /// frame after the start of its program reads.
    pub(in crate::app) fn set_record(&mut self, record: Option<OrchestratorConversationState>) {
        self.record = record;
    }

    /// Forgets the record, which the dock does when the session that it belongs to
    /// goes. The conversation itself leaves with the other conversations of the
    /// dock.
    pub(in crate::app) fn clear(&mut self) {
        self.record = None;
    }
}

#[cfg(test)]
mod tests {
    use horizon_core::agent_bridge::actor;
    use horizon_core::{Board, RuntimeState, WorkspaceState, new_local_id};

    use super::{FRONT_DESK, FrontDesk};

    /// A board of one workspace, whose local id is of the form that a start makes.
    fn board() -> Board {
        let workspace = WorkspaceState {
            local_id: new_local_id(),
            name: "work".to_owned(),
            ..WorkspaceState::default()
        };
        Board::from_runtime_state(&RuntimeState {
            workspaces: vec![workspace],
            ..RuntimeState::default()
        })
        .expect("the board of the test")
    }

    #[test]
    fn the_reserved_key_is_no_local_id_of_a_workspace() {
        assert_eq!(FRONT_DESK, "front-desk");
        assert!(
            !actor::is_uuid(FRONT_DESK),
            "a workspace local id is a UUID, so the two kinds of key cannot meet"
        );
        let board = board();
        let local_id = board.workspaces[0].local_id.clone();
        assert_ne!(FRONT_DESK, local_id);
    }

    #[test]
    fn a_board_without_an_active_workspace_shows_the_front_desk() {
        let mut front_desk = FrontDesk::default();
        front_desk.show_workspace();
        let mut board = board();
        board.active_workspace = None;

        assert!(front_desk.shows(&board));
        assert_eq!(front_desk.key(&board), FRONT_DESK);
    }
}
