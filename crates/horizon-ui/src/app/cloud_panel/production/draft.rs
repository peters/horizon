//! What a dismissing New cloud dialog keeps and what it stops: the choices stay, its view
//! state goes, and the card in its workspace shows what the draft was doing.
use super::{HorizonApp, Production, creation, repository_setup};
use egui::Context;

impl Production {
    /// The workspace whose cloud the open dialog makes: where a dismissal parks its draft.
    pub(in crate::app) fn creation_workspace(&self) -> Option<String> {
        self.launch.workspace.clone()
    }

    /// The draft as its card says it: a title, the repository, and what it does now.
    pub(in crate::app) fn draft_card(&self) -> (String, String, &'static str) {
        let title = self.title.trim();
        let title = if title.is_empty() {
            "New cloud".to_owned()
        } else {
            title.to_owned()
        };
        let status = if self.pending_creation.is_some() {
            "Checking this repository before it starts"
        } else if self.launch.submitted || self.launch.watch.is_some() {
            "Starting the cloud"
        } else if repository_setup::running_here(self) {
            "Its setup agent is working; the agent's panel stays open"
        } else {
            "Not started. Continue to finish the choices."
        };
        (title, repository_line(self), status)
    }
}

/// What a parked draft says when its queued start needs cloud accounts.
pub(in crate::app) const PARKED_START_NEEDS_ACCOUNTS: &str =
    "Starting this cloud needs cloud accounts. Continue and sign in, then start it again.";

impl HorizonApp {
    /// The account gate of a queued start. An open dialog opens the cloud settings, as it
    /// always did; a parked draft says on the board what its start needs instead, because a
    /// dialog that no one asked for would cover the workspace the person returned to.
    pub(in crate::app) fn cloud_start_account_gate(&mut self, ctx: &Context) {
        self.cloud_prototype.production.launch.submitted = false;
        if self.cloud_prototype.production.draft.is_some() {
            self.cloud_prototype.error = Some(PARKED_START_NEEDS_ACCOUNTS.to_owned());
        } else {
            self.open_cloud_accounts(ctx, true);
            self.cloud_prototype.production.launch.submitted = true;
        }
    }
}

#[cfg(test)]
impl Production {
    /// Names the draft, as typing in the open dialog does.
    pub(in crate::app) fn set_draft_title(&mut self, title: &str) {
        self.title = title.to_owned();
    }
}

/// The repository the draft names, in one line.
fn repository_line(form: &Production) -> String {
    let repository = form.repository.trim();
    if repository.is_empty() {
        return "No repository chosen yet".to_owned();
    }
    let name = std::path::Path::new(repository)
        .file_name()
        .map_or_else(|| repository.to_owned(), |name| name.to_string_lossy().into_owned());
    format!("{name} · nothing rented yet")
}

impl HorizonApp {
    /// The work a parked draft goes on doing each frame: a stock watch it armed, its image
    /// publishing form, and the repository check a Start began. A parked draft is not a
    /// closed one, so none of them stops while its card shows.
    pub(in crate::app) fn poll_parked_cloud_draft(&mut self, ctx: &Context) {
        let root = self.cloud_prototype.root.clone();
        let form = &mut self.cloud_prototype.production;
        creation_draft_ticks(form, root.as_deref(), ctx);
        // A watch submits `launch.submitted`; the launch dispatch turns it into the cloud, as
        // it does while the dialog is open.
        self.poll_cloud_launch(ctx);
        self.poll_cloud_creation(ctx);
    }
}

/// The parts of the dialog's per-frame work that outlive its view, in the dialog's order: a
/// stock watch, the launch it submits, and the repository check a Start began.
fn creation_draft_ticks(form: &mut Production, root: Option<&std::path::Path>, ctx: &Context) {
    creation::watch::poll(form);
    creation::publishing::update(form, root, ctx);
}
