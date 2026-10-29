//! What a dismissing New cloud dialog keeps and what it stops: the choices stay, its view
//! state goes, and the card in its workspace shows what the draft was doing. The choices of a
//! parked draft are saved beside the cloud settings, so a quit or a session switch brings the
//! card back.
use super::{HorizonApp, Production, creation, draft_store::Failure, repository_setup};
use crate::app::cloud_panel::new_cloud_draft::Draft;
use egui::Context;
use horizon_core::{
    PanelKind, WorkspaceId,
    cloud_runtime::{
        draft::{Kept, Worker},
        provider,
    },
};
use std::path::Path;

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

    /// The choices this dialog keeps when it is parked, owned by `session`. What it is doing,
    /// such as a clone, a token, a check, a publishing selection, a sibling or a pending
    /// creation, is not a choice of the person and is never kept.
    pub(in crate::app) fn kept_draft(&self, session: Option<String>) -> Kept {
        Kept {
            session,
            title: self.title.clone(),
            repository: self.repository.clone(),
            revision: self.revision.clone(),
            profile: self.selected_profile.clone(),
            worker: self.size.map(|(cpu, memory_gb)| Worker { cpu, memory_gb }),
            placement: self.placement.clone(),
            provider: self.provider.map(|provider| provider.id.to_owned()),
            tailnet: self.tailnet.clone(),
            setup_agent: self
                .setup_agent
                .and_then(horizon_core::agent_definition)
                .map(|definition| definition.id.to_owned()),
            setup_preset: self.setup_preset.clone(),
            configuration: horizon_core::cloud_runtime::draft::Configuration::of(self.launch.configuration),
            new_workspace: self.new_workspace.is_some(),
        }
    }

    /// Applies the choices a saved draft kept. A provider or a setup agent that this Horizon no
    /// longer knows falls back to what the dialog offers by itself, as a profile that the
    /// repository no longer has falls back to its default when the settings are read.
    pub(in crate::app) fn apply_kept_draft(&mut self, kept: &Kept) {
        self.title.clone_from(&kept.title);
        self.repository.clone_from(&kept.repository);
        self.revision.clone_from(&kept.revision);
        self.selected_profile.clone_from(&kept.profile);
        self.size = kept.worker.map(|worker| (worker.cpu, worker.memory_gb));
        self.placement.clone_from(&kept.placement);
        self.provider = kept.provider.as_deref().and_then(provider::by_id);
        self.tailnet.clone_from(&kept.tailnet);
        self.setup_agent = kept.setup_agent.as_deref().and_then(agent_kind);
        self.setup_preset.clone_from(&kept.setup_preset);
        self.launch.configuration = kept.configuration.launch();
    }
}

#[cfg(test)]
impl Production {
    /// Names the draft's repository as choosing one in the dialog does, without reading it.
    pub(in crate::app) fn set_draft_repository(&mut self, repository: &str) {
        self.repository = repository.to_owned();
    }
}

/// The setup agent that `id` names, such as `claude`.
fn agent_kind(id: &str) -> Option<PanelKind> {
    horizon_core::all_agent_kinds()
        .iter()
        .copied()
        .find(|kind| horizon_core::agent_definition(*kind).is_some_and(|definition| definition.id == id))
}

impl HorizonApp {
    /// Keeps the choices of the draft that was just parked for the next start and the next
    /// session: the card of that workspace comes back with them. The choices stay in memory for
    /// this session, and a worker thread writes them, so the frame waits for no disk.
    pub(in crate::app) fn keep_cloud_draft_record(&mut self, ctx: &Context) {
        let (Some(root), Some(workspace)) = (
            self.cloud_prototype.root.clone(),
            self.cloud_prototype.production.creation_workspace(),
        ) else {
            return;
        };
        let session = self.active_session.as_ref().map(|session| session.session_id.clone());
        let kept = self.cloud_prototype.production.kept_draft(session);
        let drafts = &mut self.cloud_prototype.production.drafts;
        drafts.wake_with(ctx);
        if let Err(error) = drafts.keep(&root, &workspace, Some(kept), Failure::Board) {
            self.cloud_prototype.error = Some(format!(
                "This draft stays in this session only; its choices were not saved: {error}"
            ));
        }
    }

    /// Forgets the saved choices of a draft that ended, so the next start shows no card for it.
    pub(in crate::app) fn forget_cloud_draft_record(&mut self) {
        let (Some(root), Some(workspace)) = (
            self.cloud_prototype.root.clone(),
            self.cloud_prototype.production.creation_workspace(),
        ) else {
            return;
        };
        if let Err(error) = self
            .cloud_prototype
            .production
            .drafts
            .keep(&root, &workspace, None, Failure::Log)
        {
            tracing::warn!(%error, %workspace, "an ended cloud draft keeps its saved choices");
        }
    }

    /// Collects what the worker threads did: the records that a worker thread read from the
    /// cloud settings folder, the cards those records hold, and a save that failed. The frame
    /// opens no file itself.
    pub(in crate::app) fn restore_cloud_drafts(&mut self, ctx: &Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let session = self.active_session.as_ref().map(|session| session.session_id.clone());
        let drafts = &mut self.cloud_prototype.production.drafts;
        drafts.wake_with(ctx);
        match drafts.poll() {
            Some((Failure::Board, error)) => {
                self.cloud_prototype.error = Some(format!(
                    "This draft stays in this session only; its choices were not saved: {error}"
                ));
            }
            Some((Failure::Log, error)) => {
                tracing::warn!(%error, "a cloud draft record is not saved");
            }
            None => {}
        }
        drafts.read(&root, session.as_deref());
        // No workspace is a session whose board is not installed yet, not a board of none.
        if !self.cloud_prototype.production.drafts.restore_pending() || self.board.workspaces.is_empty() {
            return;
        }
        let listed: Vec<String> = self
            .cloud_prototype
            .production
            .drafts
            .kept()
            .workspaces()
            .map(str::to_owned)
            .collect();
        let mut shown = None;
        let mut went = Vec::new();
        for workspace in listed {
            let kept = self
                .cloud_prototype
                .production
                .drafts
                .kept()
                .get(&workspace)
                .filter(|kept| kept.session.as_ref() == session.as_ref())
                .cloned();
            match self.board.workspace_id_by_local_id(&workspace) {
                Some(workspace_id) => {
                    if shown.is_none()
                        && let Some(kept) = kept
                    {
                        shown = Some((workspace, workspace_id, kept));
                    }
                }
                // The workspace went with this session's board, so its card has no place.
                None if kept.is_some() => went.push(workspace),
                None => {}
            }
        }
        for workspace in went {
            if let Err(error) = self
                .cloud_prototype
                .production
                .drafts
                .keep(&root, &workspace, None, Failure::Log)
            {
                tracing::warn!(%error, %workspace, "a removed workspace keeps its cloud draft");
            }
        }
        self.cloud_prototype.production.drafts.restored();
        // The person's own dialog wins over a card that a record would put back.
        if self.cloud_prototype.production.creating || self.cloud_prototype.production.draft.is_some() {
            return;
        }
        if let Some((workspace, workspace_id, kept)) = shown {
            self.show_kept_cloud_draft(ctx, &workspace, workspace_id, &kept, &root);
        }
    }

    /// Puts the card of a kept draft back in its workspace. The dialog stays closed until
    /// Continue, so a restored draft neither reads nor rents anything by itself.
    fn show_kept_cloud_draft(
        &mut self,
        ctx: &Context,
        workspace: &str,
        workspace_id: WorkspaceId,
        kept: &Kept,
        root: &Path,
    ) {
        // The card is all this workspace holds, and the frame's cleanup would take it away.
        self.board.retain_workspace_when_empty(workspace_id);
        let position = self.cloud_prototype.groups.next_position(workspace, &self.board);
        let session = self.active_session.as_ref().map(|session| session.session_id.clone());
        let made = kept
            .new_workspace
            .then(|| super::new_workspace::Intent::restored(workspace_id, Some(root.to_path_buf()), ctx));
        let form = &mut self.cloud_prototype.production;
        form.apply_kept_draft(kept);
        form.draft = Some(Draft {
            workspace: Some(workspace.to_owned()),
            position,
            restored: true,
        });
        form.creating = false;
        form.launch.restore_draft(workspace, session);
        form.new_workspace = made;
    }

    /// Whether a save of the drafts still runs: the exit waits the frames of it, within its own
    /// limit, so a draft that a frame just parked reaches the file.
    pub(in crate::app) fn cloud_drafts_hold_exit(&self) -> bool {
        self.cloud_prototype.production.drafts.saving()
    }

    /// Whether a frame keeps its empty workspaces: the records of this session, which may hold a
    /// card for one, are still being read or their card is not placed yet.
    pub(in crate::app) fn cloud_drafts_hold_workspaces(&self) -> bool {
        self.cloud_prototype.production.drafts.unsettled()
    }

    /// Reads the repository of a restored draft when Continue opens its dialog, so that the
    /// choices it kept, such as the profile, apply to the settings that arrive.
    pub(in crate::app) fn read_restored_cloud_draft(&mut self, ctx: &egui::Context) {
        let form = &self.cloud_prototype.production;
        if form.profiles.is_none() && !form.repository.trim().is_empty() {
            self.read_cloud_profiles(ctx);
        }
    }
}

#[cfg(test)]
impl HorizonApp {
    /// The record that the drafts in memory hold for `workspace`: a parked or restored card
    /// without a read of the file.
    pub(in crate::app) fn cloud_draft_record(&self, workspace: &str) -> Option<&Kept> {
        self.cloud_prototype.production.drafts.kept().get(workspace)
    }

    /// Whether the records of the active session are read and no card of them waits for its
    /// board.
    pub(in crate::app) fn cloud_drafts_settled(&self) -> bool {
        let session = self.active_session.as_ref().map(|session| session.session_id.as_str());
        self.cloud_prototype.production.drafts.settled(session)
    }

    /// Waits for the saves the frames queued, so that a test reads the file the worker wrote.
    pub(in crate::app) fn settle_cloud_drafts(&mut self) {
        self.cloud_prototype.production.drafts.settle();
    }

    /// Holds each save on the writer thread until the test sends on the returned channel, so
    /// that a test can show a frame that parks a draft writes no file. Call it before the first
    /// save.
    pub(in crate::app) fn hold_cloud_draft_saves(&mut self) -> std::sync::mpsc::Sender<()> {
        self.cloud_prototype.production.drafts.hold_saves()
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
