//! The notices of the front desk that name the state of a work item: one notice for
//! each work item that goes into a state that the person waits for.

use horizon_core::workspace_list::Orchestrator;

use crate::app::HorizonApp;

use super::super::FRONT_DESK;

/// The most characters of the name of a work item in a notice of the front desk.
const WORK_ITEM_NOTICE_NAME_CHARS: usize = 40;

/// The word of the notice for a turn that ended while the dock showed it: the
/// word that [`Orchestrator::Done`] gives a turn that ended unseen.
const FINISHED_WORD: &str = "finished";

impl HorizonApp {
    /// Writes one notice in the conversation of the front desk for each work item
    /// that goes into a state that the person waits for. Horizon writes the notice:
    /// it is no turn of a program.
    pub(super) fn note_work_item_states(&mut self) {
        let work_items = self.orchestrator_dock.work_items();
        let board = &self.board;
        self.orchestrator_dock
            .retain_work_items(|work_item| board.workspace_id_by_local_id(work_item).is_some());
        for (work_item, name) in work_items {
            let Some(workspace) = board.workspace_id_by_local_id(&work_item) else {
                continue;
            };
            let state = self.workspace_orchestrator(workspace);
            let Some(before) = self.orchestrator_dock.observe_work_item(&work_item, state) else {
                continue;
            };
            // A turn that ends while the dock shows its conversation leaves no word
            // on the row, because the person saw it end. The front desk did not see
            // it, so the end of that turn is still one `finished` notice there.
            let seen_end = before == Orchestrator::Working && state == Orchestrator::Quiet;
            let word = match state.notice_word() {
                Some(word) => word,
                None if seen_end => FINISHED_WORD,
                None => continue,
            };
            let name: String = name.chars().take(WORK_ITEM_NOTICE_NAME_CHARS).collect();
            self.orchestrator_dock
                .session_notice(FRONT_DESK, &format!("{name} {word}"));
        }
    }
}
