//! Which caller asked for a card, and which workspace its cards stand in.
//!
//! A card request names its actor and no workspace. The actor settles the
//! caller: an agent panel of this board, the orchestrator that the dock runs for
//! one workspace, or a session of a machine of the list. The workspace of the
//! caller is the whole right of it, so `owner_workspace` answers it for the cards
//! of one owner, and a card of another workspace is never readable through it.
//!
//! Nothing here changes the board or reads the disk: the checks read the
//! manifest of the request, the board and the machine list that the frame holds.

use horizon_core::WorkspaceId;
use horizon_core::agent_bridge::actor;
use horizon_core::browser::manifest::{
    self,
    block::{Outcome, Request},
};
use horizon_core::node_cards;

use super::super::HorizonApp;
use super::super::browser_requests::{ActorPanel, actor_panel};
use super::request_expired;

/// Who asked for a card: an agent panel, the orchestrator of a workspace, or a
/// session of a machine of the list.
#[derive(Clone, Copy)]
pub(in crate::app) enum CardCaller<'a> {
    Panel(ActorPanel),
    /// The program that the dock runs for one workspace, which has no panel.
    Orchestrator {
        workspace: WorkspaceId,
    },
    Session {
        machine: &'a str,
    },
}

impl CardCaller<'_> {
    /// The agent panel of this caller, when a panel of this board asked. The two
    /// callers without a panel answer `None` here and are refused by every
    /// operation that needs the panel itself, such as `open_terminal`.
    pub(in crate::app) fn panel(self) -> Option<ActorPanel> {
        match self {
            Self::Panel(panel) => Some(panel),
            Self::Orchestrator { .. } | Self::Session { .. } => None,
        }
    }
}

/// The refusal of a caller that cannot ask for what it asks, such as a session of
/// a machine that asks outside the cards.
pub(in crate::app) fn caller_unavailable() -> Outcome {
    Outcome::failed("caller_unavailable", "The agent that asked is no longer in this host")
}

impl HorizonApp {
    /// The checks every request passes before Horizon applies it: the host, the
    /// deadline, and the caller, a panel of this board or a session of a machine.
    pub(in crate::app) fn request_caller<'a>(&self, request: &'a Request) -> Result<CardCaller<'a>, Outcome> {
        if request.host_instance != manifest::host_instance() {
            return Err(Outcome::failed("wrong_host", "Request belongs to another Horizon host"));
        }
        if request.deadline_at_millis <= manifest::now_millis() {
            return Err(request_expired());
        }
        if let Some(panel) = actor_panel(&self.board, &request.actor) {
            return Ok(CardCaller::Panel(panel));
        }
        // The orchestrator of a workspace asks as `horizon:orchestrator/<local
        // id>`. The id in the actor is checked against this board, and that
        // workspace is the whole right of this caller: it has no panel.
        if let Some(local_id) = actor::workspace_of(&request.actor) {
            let workspace = self
                .board
                .workspace_id_by_local_id(local_id)
                .ok_or_else(caller_unavailable)?;
            return Ok(CardCaller::Orchestrator { workspace });
        }
        let (machine, _session) = node_cards::actor_parts(&request.actor).ok_or_else(caller_unavailable)?;
        if self.machine_list.find(machine).is_none() {
            return Err(caller_unavailable());
        }
        Ok(CardCaller::Session { machine })
    }

    /// The workspace of the cards of `owner`: its panel's, the orchestrator's
    /// workspace, or its machine's.
    pub(in crate::app) fn owner_workspace(&self, owner: &str) -> Option<WorkspaceId> {
        if let Some(panel) = actor_panel(&self.board, owner) {
            return Some(panel.workspace_id);
        }
        if let Some(local_id) = actor::workspace_of(owner) {
            return self.board.workspace_id_by_local_id(local_id);
        }
        let (machine, _session) = node_cards::actor_parts(owner)?;
        self.board
            .workspaces
            .iter()
            .find(|workspace| workspace.name == machine)
            .map(|workspace| workspace.id)
    }
}
