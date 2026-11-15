//! Which panels of the board an agent panel controls.
//!
//! Horizon keeps three facts that name the agent behind a panel:
//!
//! - the owner of a card, [`crate::BlockPanelState::owner_actor`], which the
//!   tool that creates the card sets to the `horizon:<local_id>` actor of the
//!   calling agent panel;
//! - the owner of a browser panel, [`crate::browser::BrowserPanelState::owner`],
//!   which the driver reads from the live claim of the agent that drives the
//!   page, in the same actor form;
//! - the opener of a terminal or of a preset panel that an agent opened for the
//!   person: [`crate::Panel::opened_by_actor`], the `horizon:<local_id>` actor of
//!   the agent panel that asked. The display name of the kind of that panel,
//!   [`crate::Panel::opened_by`], is only the text of the chip of the titlebar and
//!   names no pair.
//!
//! Every one of the three names one panel of the board with no ambiguity. A
//! person who takes a browser panel over from an agent clears the claim, so that
//! panel loses its pair at once and needs no special rule here.
//!
//! Every pair is derived from the board on each call. Nothing is stored for it.
//! The owner of a card survives a restart, because it is saved with the panel,
//! and so does the mark of an opened panel. The owner of a browser panel does
//! not: it lives in the heartbeat of a manifest, so a restarted Horizon draws no
//! line for a page until an agent claims it again.
//!
//! Two panel kinds that agents control have no such fact here, so they are left
//! out: a Device viewer, whose owner is a field of the UI caches and not of the
//! board, and a card that Horizon wrote itself, which has no owner at all.

use crate::board::Board;
use crate::panel::PanelId;
use crate::workspace::WorkspaceId;

/// One pair of the board: an agent panel and a panel that it controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentLink {
    /// The agent panel, at the inner end of the line.
    pub agent: PanelId,
    /// The panel whose owner or opener is `agent`, at the outer end of the line.
    pub target: PanelId,
    /// The place of `agent` among the agent panels of its workspace, in board
    /// order. A workspace accent of this index gives each agent of a workspace
    /// its own colour.
    pub accent: usize,
}

impl Board {
    /// Fill `out` with the pairs of this board. A caller that keeps the vector
    /// makes no allocation for a board whose pairs do not grow.
    pub fn fill_agent_links(&self, out: &mut Vec<AgentLink>) {
        out.clear();
        for (index, agent) in self.panels.iter().enumerate() {
            if !agent.kind.is_agent() {
                continue;
            }
            let first = out.len();
            for target in self.panels.iter().filter(|target| target.id != agent.id) {
                if owner_agent(&self.panels, target) == Some(agent.id) {
                    out.push(AgentLink {
                        agent: agent.id,
                        target: target.id,
                        accent: 0,
                    });
                }
            }
            if out.len() == first {
                continue;
            }
            let accent = accent_of(&self.panels, index, agent.workspace_id);
            for link in &mut out[first..] {
                link.accent = accent;
            }
        }
    }

    /// Whether any panel of this board carries an owner or an opener actor at all.
    /// A board without one has no pair, so a caller pays this scan and nothing
    /// else. It reads one field of each panel and allocates nothing.
    #[must_use]
    pub fn has_agent_links(&self) -> bool {
        self.panels.iter().any(|panel| {
            panel.block().is_some_and(|block| block.owner_actor.is_some())
                || panel.browser().is_some_and(|browser| browser.owner.is_some())
                || panel.opened_by_actor().is_some()
        })
    }
}

/// The agent panel that controls `target`, from the facts that the board holds.
///
/// A panel with a saved owner names no pair when that owner is not an agent panel
/// of this board any more, so a gone owner draws nothing. A panel that an agent
/// opened with no actor, which a session of a build before the actor saved, names
/// no opener until an agent opens a panel in it again.
fn owner_agent(panels: &[crate::Panel], target: &crate::Panel) -> Option<PanelId> {
    let actor = target
        .block()
        .and_then(|block| block.owner_actor.as_deref())
        .or_else(|| target.browser().and_then(|browser| browser.owner.as_deref()))
        .or_else(|| target.opened_by_actor())?;
    panel_of_actor(panels, actor)
}

/// The prefix of the actor that `crate::browser_actor` writes for a panel. A test
/// below keeps this value in step with that function.
const ACTOR_PREFIX: &str = "horizon:";

/// The agent panel that the `horizon:<local_id>` actor names, when it is an
/// agent panel of this board.
///
/// `browser_actor` hashes an id that is empty, too long or holds a control
/// character, and Horizon makes no such id: it makes a UUID. A pair that such a
/// value names is not drawn, because the id is not a field of any panel here.
fn panel_of_actor(panels: &[crate::Panel], actor: &str) -> Option<PanelId> {
    let local_id = actor.strip_prefix(ACTOR_PREFIX)?;
    panels
        .iter()
        .find(|panel| panel.kind.is_agent() && panel.local_id == local_id)
        .map(|panel| panel.id)
}

/// The place of the agent panel at `index` among the agent panels of
/// `workspace`, in board order.
fn accent_of(panels: &[crate::Panel], index: usize, workspace: WorkspaceId) -> usize {
    panels[..index]
        .iter()
        .filter(|panel| panel.kind.is_agent() && panel.workspace_id == workspace)
        .count()
}

#[cfg(test)]
mod tests;
