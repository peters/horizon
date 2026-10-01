//! Which workspaces the assistant is looking at. The assistant belongs to no
//! workspace: by default it reaches all of them, and the person can narrow a
//! question to one or several. The choice is kept by workspace id, so it
//! survives workspaces being renamed or reordered.

use horizon_core::Reach;

use super::HorizonApp;

/// Every workspace, or only the ones named.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Scope {
    only: Option<Vec<String>>,
}

impl Scope {
    pub(super) fn is_all(&self) -> bool {
        self.only.is_none()
    }

    pub(super) fn includes(&self, local_id: &str) -> bool {
        self.only.as_ref().is_none_or(|ids| ids.iter().any(|id| id == local_id))
    }

    pub(super) fn set_all(&mut self) {
        self.only = None;
    }

    /// Adds or removes one workspace. Choosing from "all" narrows to that one;
    /// emptying the choice, or naming every workspace, goes back to "all".
    pub(super) fn toggle(&mut self, local_id: &str, every: &[String]) {
        match self.only.as_mut() {
            None => self.only = Some(vec![local_id.to_string()]),
            Some(ids) => {
                if let Some(position) = ids.iter().position(|id| id == local_id) {
                    ids.remove(position);
                } else {
                    ids.push(local_id.to_string());
                }
                let complete = every.iter().all(|id| ids.contains(id));
                if ids.is_empty() || complete {
                    self.only = None;
                }
            }
        }
    }

    /// Narrows to one workspace.
    pub(super) fn set_only(&mut self, local_id: &str) {
        self.only = Some(vec![local_id.to_string()]);
    }

    /// How many workspaces are named, or `None` for all of them.
    pub(super) fn count(&self) -> Option<usize> {
        self.only.as_ref().map(Vec::len)
    }
}

impl HorizonApp {
    /// What the assistant's `agent_panels` calls can reach right now.
    pub(in crate::app) fn assistant_reach(&self) -> Reach {
        if self.assistant.scope.is_all() {
            return Reach::All;
        }
        Reach::Only(
            self.board
                .workspaces
                .iter()
                .filter(|workspace| self.assistant.scope.includes(&workspace.local_id))
                .map(|workspace| workspace.id)
                .collect(),
        )
    }

    /// Short text for the scope chip.
    pub(super) fn scope_label(&self) -> String {
        match self.assistant.scope.count() {
            None => "All workspaces".to_string(),
            Some(1) => self
                .board
                .workspaces
                .iter()
                .find(|workspace| self.assistant.scope.includes(&workspace.local_id))
                .map_or_else(|| "1 workspace".to_string(), |workspace| workspace.name.clone()),
            Some(count) => format!("{count} workspaces"),
        }
    }

    pub(super) fn workspace_local_ids(&self) -> Vec<String> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| workspace.local_id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests;
