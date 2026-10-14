//! Upkeep's cloud: Start Upkeep in New Upkeep makes an always-on quick-start cloud on
//! Hetzner from the first chosen repository, with the Upkeep board beside its card. The
//! repository is cloned to this computer first when it is not here yet; the workspace, the
//! card and the board appear once it is. [`sync`] keeps the cloud in step with its worker.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{TryRecvError, channel};

use horizon_core::cloud_panel::{CloudLaunch, Placement};
use horizon_core::cloud_runtime::{self, Cancellation, repository, settings::Settings};
use horizon_core::upkeep::worker::{self, Config};
use horizon_core::{PanelKind, PanelOptions, WorkspaceId};

use super::super::{HorizonApp, creation_job};
use crate::app::upkeep::{Prepared, Preparing};

mod sync;

/// The worker every Upkeep runs on, as its board names it.
const WORKER: &str = "Hetzner, always on";
/// The name of an Upkeep's workspace, cloud and board.
const NAME: &str = "Upkeep";
/// Why a start needs a saved session: a cloud is recorded with the session that holds it.
const SAVED_SESSION: &str = "Open a saved session from Sessions before starting a cloud.";

impl HorizonApp {
    /// The folder of this computer's cloud settings, once cloud workspaces are ready.
    pub(in crate::app) fn ready_cloud_root(&self) -> Option<PathBuf> {
        self.cloud_launch_ready()
            .then(|| self.cloud_prototype.root.clone())
            .flatten()
    }

    /// Starts an Upkeep with `config` in `workspace`, or in a new workspace: prepares its
    /// first repository in the background, and saves the choice under a new cloud id.
    /// [`Self::poll_upkeep_start`] adds the cloud once the repository is ready.
    /// # Errors
    /// Why the start cannot begin: no cloud settings, or a board that no saved session holds.
    pub(in crate::app) fn start_upkeep(
        &mut self,
        ctx: &egui::Context,
        workspace: Option<WorkspaceId>,
        mut config: Config,
    ) -> Result<(), String> {
        if self.upkeep.preparing.is_some() {
            return Err("An Upkeep is still starting. Wait until its cloud appears.".into());
        }
        // As New cloud: a cloud is recorded with the session that holds its workspace.
        if !self.active_session.as_ref().is_some_and(|session| session.persistent) {
            return Err(SAVED_SESSION.into());
        }
        let root = self
            .cloud_prototype
            .root
            .clone()
            .ok_or("Cloud workspaces are not ready on this computer.")?;
        config.worker = WORKER.into();
        config.validate().map_err(|error| format!("{error}."))?;
        let cloud_id = cloud_runtime::new_id();
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let saved_as = cloud_id.clone();
        std::thread::spawn(move || {
            let _ = tx.send(prepare(&root, &saved_as, &config));
            repaint.request_repaint();
        });
        self.upkeep.preparing = Some(Preparing {
            receiver: rx,
            cloud_id,
            session: self.active_session.as_ref().map(|session| session.session_id.clone()),
            workspace,
        });
        Ok(())
    }

    /// Each frame: adds the cloud of a start whose repository is ready, and keeps every
    /// Upkeep cloud of this board in step with its worker.
    pub(in crate::app::cloud_panel) fn poll_upkeeps(&mut self, ctx: &egui::Context) {
        self.poll_upkeep_start(ctx);
        self.sync_upkeeps(ctx);
    }

    fn poll_upkeep_start(&mut self, ctx: &egui::Context) {
        let Some(preparing) = &self.upkeep.preparing else {
            return;
        };
        let prepared = match preparing.receiver.try_recv() {
            Ok(prepared) => prepared,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("Preparing the first repository stopped.".into()),
        };
        let Some(preparing) = self.upkeep.preparing.take() else {
            return;
        };
        let session = self.active_session.as_ref().map(|session| session.session_id.clone());
        let added = prepared
            .and_then(|prepared| {
                // The cloud belongs to the session the person started it in.
                if preparing.session == session {
                    Ok(prepared)
                } else {
                    Err("The session changed while the repository was prepared. Start the Upkeep again.".into())
                }
            })
            .and_then(|prepared| self.add_upkeep_cloud(ctx, &preparing.cloud_id, preparing.workspace, prepared));
        if added.is_err() {
            // No cloud holds the saved choice, so nothing would ever read it.
            let folder = worker::directory(&preparing.cloud_id);
            std::thread::spawn(move || {
                let _ = std::fs::remove_dir_all(folder);
            });
        }
        self.upkeep_started(added);
    }

    /// Adds the cloud's card and the board to `workspace`, or to a new Upkeep workspace when
    /// it is `None` or was removed, and starts the cloud.
    fn add_upkeep_cloud(
        &mut self,
        ctx: &egui::Context,
        cloud_id: &str,
        workspace: Option<WorkspaceId>,
        prepared: Prepared,
    ) -> Result<(), String> {
        // The session can have changed while the repository was prepared.
        if !self.active_session.as_ref().is_some_and(|session| session.persistent) {
            return Err(SAVED_SESSION.into());
        }
        let quick = repository::launch::quick_start::builtin().map_err(|error| error.to_string())?;
        let profile = quick
            .profiles
            .get(repository::launch::quick_start::PROFILE)
            .ok_or("The quick-start profile is missing.")?;
        let profile = creation_job::launch_profile(profile, Some(&cloud_runtime::provider::HETZNER), None)
            .map_err(|error| format!("{error}."))?;
        let launch = CloudLaunch {
            deployment_started: false,
            id: cloud_id.to_owned(),
            revision: prepared.revision.clone(),
            profile_name: repository::launch::quick_start::PROFILE.into(),
            profile,
            placement: Placement::default(),
        };
        let existing = workspace.filter(|id| self.board.workspace(*id).is_some());
        let workspace = existing.unwrap_or_else(|| self.create_workspace_visible(ctx, NAME));
        let local = self
            .board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone())
            .ok_or("The Upkeep workspace could not be made.")?;
        let id = match self.add_cloud_group(NAME.into(), local, prepared.path, launch, Vec::new(), None) {
            Ok(id) => id,
            Err(error) => {
                if existing.is_none() {
                    self.board.remove_empty_workspace(workspace);
                }
                return Err(format!("{error}."));
            }
        };
        let status = worker::directory(cloud_id).join("status.json");
        if let Err(error) = self.board.create_panel(
            PanelOptions {
                name: Some(NAME.into()),
                kind: PanelKind::Upkeep,
                command: Some(status.display().to_string()),
                size: Some([1040.0, 780.0]),
                ..PanelOptions::default()
            },
            workspace,
        ) {
            tracing::warn!(%error, "could not open the Upkeep board");
        }
        self.start_production_deployment(id, ctx);
        // The first run starts once the loop is installed, not at the next full period; the
        // marker saved with the choice keeps it across a restart until it is sent.
        let signals = &mut self.upkeep.syncs.entry(cloud_id.to_owned()).or_default().signals;
        signals.run_now = true;
        signals.run_token = Some(prepared.run_token);
        // The saved choices are read again, with the new one; a read under way may be older.
        self.upkeep.saved = None;
        self.upkeep.reading = None;
        self.upkeep.read_after = None;
        self.fit_workspace_visible(ctx, workspace);
        self.mark_runtime_dirty();
        Ok(())
    }
}

/// Makes the first repository ready on this computer and saves the choice for `cloud_id`.
/// Blocks on Git and the disk: runs on its own thread.
fn prepare(root: &Path, cloud_id: &str, config: &Config) -> Result<Prepared, String> {
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    if settings.hetzner.is_none() {
        return Err("Add a Hetzner project in Cloud settings first: Upkeep runs on Hetzner.".into());
    }
    let primary = config.primary().ok_or("Choose at least one repository.")?;
    let path = checkout(root, &settings, primary)?;
    let revision = repository::resolve(&path, "HEAD").map_err(|error| format!("{error}."))?;
    let run_token = worker::save(cloud_id, config)
        .and_then(|_| worker::mark_first_run(cloud_id))
        .map_err(|error| format!("Upkeep could not save its settings: {error}."))?;
    Ok(Prepared {
        path,
        revision,
        run_token,
    })
}

/// The checkout of `name` in `~/github/<owner>/<name>`, cloned with the GitHub sign-in
/// when it is not there yet.
fn checkout(root: &Path, settings: &Settings, name: &str) -> Result<PathBuf, String> {
    let remote = repository::source::parse(name).ok_or("The first repository is not a GitHub repository.")?;
    let parent = repository::source::default_parent(&horizon_core::dir_search::expand_tilde("~"));
    let found = repository::source::existing(&parent, &remote).or_else(|| {
        let spelled = repository::source::parse(&spelled(&parent, name)?)?;
        repository::source::existing(&parent, &spelled)
    });
    if let Some(path) = found {
        return Ok(path);
    }
    let token = match &settings.github {
        Some(github) => cloud_runtime::github::host::current(root, github)
            .map_err(|error| format!("{error}."))?
            .and_then(|(_, secret)| repository::source::Token::new(&remote, secret.expose())),
        None => None,
    };
    let destination = repository::source::destination(&parent, &remote);
    repository::source::clone(
        &remote,
        &destination,
        token.as_ref(),
        &Cancellation::default(),
        &repository::source::Progress::default(),
    )
    .map_err(|failure| format!("Cloning {name} failed: {failure}"))?;
    Ok(destination)
}

/// `name` as the folders under `parent` spell it when they differ only in case: GitHub
/// names come in lowercase, and an earlier clone keeps the spelling of its owner.
fn spelled(parent: &Path, name: &str) -> Option<String> {
    let (owner, repository) = name.split_once('/')?;
    let find = |folder: &Path, wanted: &str| {
        std::fs::read_dir(folder)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|found| found.eq_ignore_ascii_case(wanted))
    };
    let owner = find(parent, owner)?;
    let repository = find(&parent.join(&owner), repository)?;
    Some(format!("{owner}/{repository}"))
}

#[cfg(test)]
mod tests;
