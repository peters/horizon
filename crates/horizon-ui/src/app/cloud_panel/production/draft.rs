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
        } else if repository_setup::running_here(self) {
            "Its setup agent is working; the agent's panel stays open"
        } else {
            "Not started. Continue to finish the choices."
        };
        (title, repository_line(self), status)
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
        self.poll_cloud_creation(ctx);
        let root = self.cloud_prototype.root.clone();
        let form = &mut self.cloud_prototype.production;
        creation_draft_ticks(form, root.as_deref(), ctx);
    }
}

/// The parts of the dialog's per-frame work that outlive its view.
fn creation_draft_ticks(form: &mut Production, root: Option<&std::path::Path>, ctx: &Context) {
    creation::watch::poll(form);
    creation::publishing::update(form, root, ctx);
}
