//! Which caller asked for a card, and which workspace its cards stand in.
//!
//! A card request names its actor and no workspace. The actor settles the
//! caller: an agent panel of this board, the orchestrator that the dock runs for
//! one workspace, or a session of a machine of the list. The workspace of the
//! caller is the whole right of it, so `owner_workspace` answers it for the cards
//! of one owner, and a card of another workspace is never readable through it.
//!
//! The front desk is one more actor of the dock, and it owns no workspace. It
//! therefore creates no card, reads no card and opens no panel, and every request
//! of that actor is refused with `caller_unavailable`.
//!
//! Nothing here changes the board or reads the disk: the checks read the
//! manifest of the request, the board, the dock and the machine list that the
//! frame holds.

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
        if actor::is_front_desk(&request.actor) {
            // The front desk belongs to no workspace, so it has no card of its own
            // and no panel to open: this slice grants it nothing.
            return Err(caller_unavailable());
        }
        if let Some(panel) = actor_panel(&self.board, &request.actor) {
            return Ok(CardCaller::Panel(panel));
        }
        // The orchestrator of a workspace asks as `horizon:orchestrator/<local
        // id>`. The identity belongs to a program that Horizon started, so the
        // host accepts it only while the dock holds a running bridge for that
        // workspace: a stale actor of a program that ended is refused, also when
        // the workspace is still on the board.
        if let Some(local_id) = actor::workspace_of(&request.actor) {
            return self
                .orchestrator_workspace(local_id)
                .map(|workspace| CardCaller::Orchestrator { workspace })
                .ok_or_else(caller_unavailable);
        }
        let (machine, _session) = node_cards::actor_parts(&request.actor).ok_or_else(caller_unavailable)?;
        if self.machine_list.find(machine).is_none() {
            return Err(caller_unavailable());
        }
        Ok(CardCaller::Session { machine })
    }

    /// The workspace of the cards of `owner`: its panel's, the orchestrator's
    /// workspace, or its machine's. The front desk owns no workspace, so it reads
    /// no card.
    pub(in crate::app) fn owner_workspace(&self, owner: &str) -> Option<WorkspaceId> {
        if actor::is_front_desk(owner) {
            return None;
        }
        if let Some(panel) = actor_panel(&self.board, owner) {
            return Some(panel.workspace_id);
        }
        if let Some(local_id) = actor::workspace_of(owner) {
            return self.orchestrator_workspace(local_id);
        }
        let (machine, _session) = node_cards::actor_parts(owner)?;
        self.board
            .workspaces
            .iter()
            .find(|workspace| workspace.name == machine)
            .map(|workspace| workspace.id)
    }

    /// The workspace of the orchestrator that `local_id` names, while the dock
    /// runs its program: the identity of that caller is the bridge, so a read of
    /// its cards follows the same rule as every other request of it.
    fn orchestrator_workspace(&self, local_id: &str) -> Option<WorkspaceId> {
        self.orchestrator_dock
            .running(local_id)
            .then(|| self.board.workspace_id_by_local_id(local_id))
            .flatten()
    }
}
