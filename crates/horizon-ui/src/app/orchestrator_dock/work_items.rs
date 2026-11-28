//! The work items of the front desk, and the row that names one: the state that
//! the pass of the dock reads of each work item, the last text of its orchestrator,
//! the cost of its turns, and the line of Horizon that goes to the program of the
//! front desk when the person starts or leaves a proposal.
//!
//! A work item is a workspace with its orchestrator conversation. Its state is the
//! word of its row of the sidebar, so the front desk reads one rule and not two.

use super::FRONT_DESK;
use super::OrchestratorDock;
use super::session::Session;

impl OrchestratorDock {
    /// The last text of the program of `local_id`, cut to `max` characters.
    pub(in crate::app) fn last_text_of(&self, local_id: &str) -> Option<String> {
        self.sessions
            .get(local_id)?
            .last_text(horizon_browser_control::manifest::block::MAX_WORK_ITEM_LAST_CHARS)
    }

    /// The cost of the ended turns of the program of `local_id`.
    pub(in crate::app) fn cost_of(&self, local_id: &str) -> f64 {
        self.sessions.get(local_id).map_or(0.0, Session::cost_usd_total)
    }

    /// The state of one work item that the front desk started, in the words of its
    /// reference row, while the board holds its workspace. The answer of the work
    /// items names the same state in the machine words of
    /// [`horizon_core::workspace_list::Orchestrator::state`].
    pub(in crate::app) fn work_item_word(&self, work_item: &str) -> Option<&'static str> {
        self.work_item_states.get(work_item).and_then(|state| state.row_word())
    }

    /// The work items that the front desk started, with their names, in the order
    /// of their reference rows.
    pub(in crate::app) fn work_items(&self) -> Vec<(String, String)> {
        self.sessions
            .get(FRONT_DESK)
            .map(|session| {
                session
                    .snapshot()
                    .entries
                    .iter()
                    .filter_map(|entry| match entry {
                        horizon_core::agent_bridge::Entry::WorkItem { work_item, name } => {
                            Some((work_item.clone(), name.clone()))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keeps the state of one work item as this frame reads it, and answers the
    /// state that the pass before it read, when they differ. The first read of a
    /// work item is no change.
    pub(in crate::app) fn observe_work_item(
        &mut self,
        work_item: &str,
        state: horizon_core::workspace_list::Orchestrator,
    ) -> Option<horizon_core::workspace_list::Orchestrator> {
        match self.work_item_states.insert(work_item.to_owned(), state) {
            Some(seen) if seen != state => Some(seen),
            _ => None,
        }
    }

    /// Forgets the work items that the board no longer holds, so the map stays as
    /// small as the board.
    pub(in crate::app) fn retain_work_items(&mut self, held: impl Fn(&str) -> bool) {
        self.work_item_states.retain(|work_item, _| held(work_item));
    }

    /// Sends one line of Horizon to the program of the front desk as a turn of its
    /// own. A conversation that runs a turn keeps the line until that turn ends.
    pub(in crate::app) fn front_desk_system_message(&mut self, line: &str) -> bool {
        self.session_mut(FRONT_DESK).system_message(line)
    }

    /// Sends the line of Horizon that waits in each conversation, when no turn of
    /// that conversation runs. It answers whether a line went out.
    pub(in crate::app) fn flush_system_messages(&mut self) -> bool {
        let mut sent = false;
        for session in self.sessions.values_mut() {
            sent |= session.flush_system();
        }
        sent
    }

    /// Compares the turns and the bridges of this pass with the pass before it, so a
    /// start, a stop and the start or the end of a turn tell the sidebar to read the
    /// words of its rows again. It answers whether one of them changed.
    pub(in crate::app) fn note_turn_changes(&mut self) -> bool {
        let watch: Vec<(String, bool, bool, usize)> = self
            .sessions
            .iter()
            .map(|(local_id, session)| {
                let turn = session.turn_snapshot();
                (local_id.clone(), turn.busy, session.has_bridge(), turn.failed)
            })
            .collect();
        if watch == self.turn_watch {
            return false;
        }
        self.turn_watch = watch;
        true
    }
}
