//! The draft of each conversation of the dock: the text that the field holds
//! between two messages of the person, and the example that a click puts there.

use super::OrchestratorDock;

impl OrchestratorDock {
    /// The draft of the workspace that `local_id` names, which is empty until the
    /// person writes in it.
    pub(super) fn draft(&self, local_id: &str) -> &str {
        self.drafts.get(local_id).map_or("", String::as_str)
    }

    /// The draft of the workspace that `local_id` names, which a test writes as a
    /// person who typed in the field would.
    #[cfg(test)]
    pub(in crate::app) fn draft_mut(&mut self, local_id: &str) -> &mut String {
        self.drafts.entry(local_id.to_owned()).or_default()
    }

    /// The text of the draft of `local_id`, when it holds more than white space.
    /// The field keeps the writing of the person: the send clears it, and a
    /// message that goes out from another path leaves it alone.
    pub(super) fn draft_text(&self, local_id: &str) -> Option<String> {
        let text = self.drafts.get(local_id)?.trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// Empties the field of `local_id`, which a message that went out does.
    pub(super) fn clear_draft(&mut self, local_id: &str) {
        self.drafts.remove(local_id);
    }

    /// Keeps the text of the example that the person clicked in the draft of
    /// `local_id`, when that draft holds no text yet. The answer says whether the
    /// draft took it: then the field takes the keyboard. The click sends nothing.
    pub(super) fn pick_example(&mut self, local_id: &str, text: &str) -> bool {
        let draft = self.drafts.entry(local_id.to_owned()).or_default();
        if !draft.trim().is_empty() {
            return false;
        }
        draft.clear();
        draft.push_str(text);
        true
    }
}
