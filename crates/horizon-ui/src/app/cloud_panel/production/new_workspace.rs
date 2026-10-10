//! New cloud for a workspace that New workspace made: the GPU profile that Cloud GPU asked
//! for, a repository that runs on This PC, the choice the person keeps for a repository, and
//! the empty workspace that goes when the dialog is cancelled.
use super::{HorizonApp, Production};
use crate::{app::util::primary_button, theme};
use egui::{Context, RichText, Ui, vec2};
use horizon_core::{
    PanelOptions, WorkspaceId,
    cloud_panel::{CloudConfig, WorkspacePlacement},
    cloud_runtime::repository_choice::Choices,
};
use std::path::PathBuf;

/// The workspace New workspace made for the cloud, and what it asked for.
#[derive(Clone, Debug)]
pub(super) struct Intent {
    workspace: WorkspaceId,
    /// Cloud GPU: the first GPU profile of each repository the dialog reads.
    gpu: bool,
    /// The repository Cloud GPU last chose a profile for. A reread of it keeps the person's
    /// pick; another repository gets its own GPU profile.
    chosen_for: Option<String>,
    /// Cloud GPU found no GPU profile, so the dialog says the cloud runs on a CPU worker.
    no_gpu: bool,
    /// The choice kept for each repository, and where they are saved.
    choices: Choices,
    root: Option<PathBuf>,
    /// Keep what the person chooses now for the dialog's repository.
    keep: bool,
    /// Why the last keep or forget was not saved.
    error: Option<String>,
}

impl HorizonApp {
    /// Opens New cloud for `workspace`, which New workspace just made; `gpu` for Cloud GPU.
    pub(in crate::app) fn open_cloud_for_new_workspace(&mut self, ctx: &Context, workspace: WorkspaceId, gpu: bool) {
        self.open_cloud_for_workspace(ctx, workspace);
        let root = self.cloud_prototype.root.clone();
        let form = &mut self.cloud_prototype.production;
        if form.creating {
            form.new_workspace = Some(Intent {
                workspace,
                gpu,
                chosen_for: None,
                no_gpu: false,
                choices: root.as_deref().map(Choices::load).unwrap_or_default(),
                root,
                keep: false,
                error: None,
            });
        }
    }

    /// Removes the workspace that New workspace made for the cloud when the dialog closes
    /// before a cloud is in it and nothing else went in: Cancel, a session switch, or any
    /// other way. The board keeps its last workspace, so the only one stays.
    pub(super) fn discard_new_cloud_workspace(&mut self) {
        let Some(intent) = self.cloud_prototype.production.new_workspace.take() else {
            return;
        };
        let Some(workspace) = self.board.workspace(intent.workspace) else {
            return;
        };
        let local = workspace.local_id.clone();
        let cloud = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .any(|group| group.workspace == local);
        if !cloud && self.board.remove_empty_workspace(intent.workspace) {
            self.mark_runtime_dirty();
        }
    }

    /// Ends an open New cloud dialog before a session switch saves the board, also while it
    /// waits behind the account setup of a first cloud, which ends with it.
    pub(in crate::app) fn close_cloud_creation_for_session_switch(&mut self) {
        let form = &mut self.cloud_prototype.production;
        let waiting = form.setup.open && form.setup.resumes_creation();
        if waiting {
            form.setup = super::setup::State::default();
        }
        if form.creating || waiting {
            self.close_cloud_creation();
        }
    }

    /// Opens the dialog's repository on This PC: a terminal in it, in the dialog's workspace,
    /// and the dialog closes.
    pub(super) fn open_repository_on_this_pc(&mut self, ctx: &Context) {
        if !self.keep_new_workspace_choice(WorkspacePlacement::Local) {
            return;
        }
        let form = &self.cloud_prototype.production;
        let repository = PathBuf::from(form.repository.trim());
        let workspace = form
            .launch
            .workspace
            .as_deref()
            .and_then(|local| self.board.workspace_id_by_local_id(local));
        // Held out of the close, which would take the still empty workspace away, until the
        // terminal is there.
        let made = self.cloud_prototype.production.new_workspace.take();
        self.close_cloud_creation();
        let Some(workspace) = workspace else {
            return;
        };
        if let Some(entry) = self.board.workspace_mut(workspace) {
            entry.cwd = Some(repository.clone());
        }
        let options = PanelOptions {
            cwd: Some(repository),
            ..PanelOptions::default()
        };
        match self.create_panel_with_options(options, workspace) {
            Ok(panel) => self.reveal_new_panel(ctx, workspace, panel),
            Err(error) => {
                tracing::error!(%error, "could not open the repository on This PC");
                // A workspace New workspace made for it stays empty, so it goes.
                if made.is_some_and(|intent| intent.workspace == workspace) {
                    self.board.remove_empty_workspace(workspace);
                }
            }
        }
        self.mark_runtime_dirty();
    }
}

/// The profile Cloud GPU asks for in `config`, read for the dialog's repository: its first
/// GPU profile, once for each repository, so a reread keeps the person's pick (`None`). With
/// no GPU profile it is the repository's default, and the dialog says the cloud runs on a
/// CPU worker.
pub(super) fn gpu_profile(form: &mut Production, config: &CloudConfig) -> Option<String> {
    let repository = form.repository.clone();
    let intent = form.new_workspace.as_mut().filter(|intent| intent.gpu)?;
    if intent.chosen_for.as_deref() == Some(repository.as_str()) {
        return None;
    }
    intent.chosen_for = Some(repository);
    let found = config
        .profiles
        .iter()
        .find(|(_, profile)| profile.gpu)
        .map(|(name, _)| name.clone());
    intent.no_gpu = found.is_none();
    Some(found.unwrap_or_else(|| config.default.clone()))
}

/// What the dialog says above its fields for a workspace from New workspace, and the This PC
/// choice of a repository whose `cloud.yml` asks for it. True when the person chose This PC.
pub(super) fn notes(ui: &mut Ui, form: &mut Production) -> bool {
    // Only about the repository the field shows now, not while another one is typed or
    // read. A kept choice needs no profile: it is about the repository.
    if form.launch.loading() || form.source.editing() || form.repository.trim().is_empty() {
        return false;
    }
    if form.profiles.is_some() && form.new_workspace.as_ref().is_some_and(|intent| intent.no_gpu) {
        ui.label(
            RichText::new(
                "This repository has no GPU profile, so the cloud runs on a CPU worker. For a GPU, add a profile \
                 with gpu: true to .horizon/cloud.yml.",
            )
            .size(12.5)
            .color(theme::PALETTE_YELLOW()),
        );
    }
    let asked = form
        .profiles
        .as_ref()
        .is_some_and(|config| config.placement == WorkspacePlacement::Local);
    let repository = form.repository.clone();
    let mut chosen = false;
    let kept = form
        .new_workspace
        .as_mut()
        .and_then(|intent| choice::row(ui, intent, &repository, asked, &mut chosen));
    // A kept choice wins over what the repository asks for.
    let local = kept.map_or(asked, |kept| kept == WorkspacePlacement::Local);
    if !local {
        return chosen;
    }
    egui::Frame::new()
        .fill(theme::PANEL_BG_ALT())
        .stroke(egui::Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("This repository runs on This PC")
                    .size(14.5)
                    .strong()
                    .color(theme::FG()),
            );
            let why = if kept.is_some() {
                "You keep This PC for it. You can still start a cloud for it."
            } else {
                "Its .horizon/cloud.yml sets placement: local. You can still start a cloud for it."
            };
            ui.label(RichText::new(why).size(12.5).color(theme::FG_DIM()));
            chosen |= ui
                .add(primary_button("Open on This PC").min_size(vec2(150.0, 30.0)))
                .clicked();
        });
    chosen
}

mod choice;
pub(super) mod machine;
#[cfg(test)]
mod tests;

impl HorizonApp {
    /// Keeps `placement` for the dialog's repository when the person asked for it, before
    /// the dialog acts on it. False when it could not be kept: the dialog then stays open
    /// and says why.
    pub(super) fn keep_new_workspace_choice(&mut self, placement: WorkspacePlacement) -> bool {
        let form = &mut self.cloud_prototype.production;
        let repository = form.repository.clone();
        form.new_workspace
            .as_mut()
            .is_none_or(|intent| keep(intent, &repository, placement))
    }
}

/// Keeps `placement` for `repository` when the person asked to keep the choice. False
/// when it could not be saved; `intent` then holds why.
pub(super) fn keep(intent: &mut Intent, repository: &str, placement: WorkspacePlacement) -> bool {
    if !intent.keep {
        return true;
    }
    let saved = match intent.root.clone() {
        Some(root) => intent
            .choices
            .set(&root, repository, Some(placement))
            .map_err(|error| error.to_string()),
        None => Err("Horizon has no cloud settings".to_owned()),
    };
    intent.error = saved
        .err()
        .map(|error| format!("The choice for this repository was not kept: {error}"));
    intent.error.is_none()
}
