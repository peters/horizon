//! The launch state of a draft that a saved record brought back, which belongs to this session.
use super::State;

impl State {
    /// Joins the draft of `workspace` to this session, so that a session switch cannot end it as
    /// another session's dialog.
    pub(in crate::app) fn restore_draft(&mut self, workspace: &str, session: Option<String>) {
        self.workspace = Some(workspace.to_owned());
        self.session = session;
    }
}
