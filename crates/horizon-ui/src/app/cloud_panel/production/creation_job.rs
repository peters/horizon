//! Resolve captured creation inputs without blocking painting or changing their destination.
use super::{CloudGroup, CloudLaunch, HorizonApp, PathBuf, cloud_runtime};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError, channel},
};

pub(super) struct Pending {
    receiver: Receiver<cloud_runtime::Result<Resolved>>,
    session: Option<String>,
    workspace: String,
    title: String,
    launch: CloudLaunch,
    cancel: CancelOnDrop,
}

struct CancelOnDrop(cloud_runtime::Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
struct WorkPermit(Arc<AtomicBool>);
impl Drop for WorkPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct Resolved {
    repository: PathBuf,
    revision: String,
}

impl super::Production {
    /// Whether the open or pending cloud creation will place its cloud in
    /// `workspace`, including while cloud settings interrupt it.
    pub(in crate::app::cloud_panel) fn creation_targets(&self, workspace: &str) -> bool {
        self.pending_creation
            .as_ref()
            .is_some_and(|pending| pending.workspace == workspace)
            || ((self.creating || self.setup.resumes_creation()) && self.launch.workspace.as_deref() == Some(workspace))
    }
}

impl HorizonApp {
    pub(super) fn create_production_cloud(&mut self, ctx: &egui::Context) -> cloud_runtime::Result<()> {
        if self.cloud_prototype.production.pending_creation.is_some() {
            return Ok(());
        }
        if self.cloud_prototype.production.launch.workspace.is_some()
            && !self.active_session.as_ref().is_some_and(|session| session.persistent)
        {
            return Err(cloud_runtime::Error::Invalid(
                "Open a saved session from Sessions before starting a cloud",
            ));
        }
        if self.cloud_prototype.production.title.trim().is_empty() {
            return Err(cloud_runtime::Error::Invalid("Enter a cloud title"));
        }
        let workspace = if let Some(local) = &self.cloud_prototype.production.launch.workspace {
            self.board
                .workspace_id_by_local_id(local)
                .ok_or(cloud_runtime::Error::Invalid("The selected workspace was removed"))?
        } else {
            self.board.ensure_workspace()
        };
        if self.workspace_is_detached(workspace) {
            return Err(cloud_runtime::Error::Invalid(
                "Move this workspace to the main window before creating a cloud",
            ));
        }
        let workspace = self
            .board
            .workspace(workspace)
            .map(|workspace| workspace.local_id.clone())
            .ok_or(cloud_runtime::Error::Invalid("No workspace selected"))?;
        let form = &self.cloud_prototype.production;
        let profile = form
            .profiles
            .as_ref()
            .and_then(|config| config.profiles.get(&form.selected_profile))
            .ok_or(cloud_runtime::Error::Invalid("Choose a repository profile"))?;
        let profile = match form.size {
            Some(size) => cloud_runtime::flavors::sized(profile, size)?,
            // A GPU profile's size is fixed; a CPU profile's own size must also be offered.
            None if profile.gpu => profile.clone(),
            None => cloud_runtime::flavors::sized(profile, (profile.cpu, profile.memory_gb))?,
        };
        let repository = horizon_core::Config::expand_tilde(&form.repository);
        let revision = if let Some(revision) = &form.launch.revision {
            revision.clone()
        } else if form.revision.is_empty() {
            "HEAD".into()
        } else {
            form.revision.clone()
        };
        let busy = form.creation_busy.clone();
        if busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(cloud_runtime::Error::Invalid(
                "Previous repository check is still stopping; retry shortly",
            ));
        }
        let permit = WorkPermit(busy);
        let cancel = cloud_runtime::Cancellation::default();
        let (sender, receiver) = channel();
        self.cloud_prototype.production.pending_creation = Some(Pending {
            receiver,
            cancel: CancelOnDrop(cancel.clone()),
            session: self.active_session.as_ref().map(|session| session.session_id.clone()),
            workspace,
            title: form.title.trim().to_owned(),
            launch: CloudLaunch {
                deployment_started: false,
                id: cloud_runtime::new_id(),
                revision: String::new(),
                profile_name: form.selected_profile.clone(),
                profile,
            },
        });
        self.cloud_prototype.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                cancel.check()?;
                let repository = repository.canonicalize()?;
                let runner = cloud_runtime::command::Runner {
                    cancel: &cancel,
                    emit: &|_| {},
                    secrets: Vec::new(),
                };
                let revision = cloud_runtime::repository::resolve_with_runner(&repository, &revision, &runner)?;
                Ok(Resolved { repository, revision })
            })();
            drop(permit);
            let _ = sender.send(result);
            ctx.request_repaint();
        });
        Ok(())
    }

    pub(super) fn poll_cloud_creation(&mut self, ctx: &egui::Context) {
        let form = &mut self.cloud_prototype.production;
        let Some(pending) = &form.pending_creation else { return };
        if !form.creating || pending.session != self.active_session.as_ref().map(|session| session.session_id.clone()) {
            pending.cancel.0.cancel();
            form.pending_creation = None;
            form.creating = false;
            return;
        }
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(TryRecvError::Disconnected) => Err(cloud_runtime::Error::Invalid("Repository validation interrupted")),
        };
        let Some(pending) = form.pending_creation.take() else {
            return;
        };
        if let Err(error) = result.and_then(|resolved| self.finish_cloud_creation(ctx, pending, resolved)) {
            self.cloud_prototype.error = Some(error.to_string());
        }
    }

    fn finish_cloud_creation(
        &mut self,
        ctx: &egui::Context,
        mut pending: Pending,
        resolved: Resolved,
    ) -> cloud_runtime::Result<()> {
        let workspace = self
            .board
            .workspace_id_by_local_id(&pending.workspace)
            .ok_or(cloud_runtime::Error::Invalid("The selected workspace was removed"))?;
        if self.workspace_is_detached(workspace) {
            return Err(cloud_runtime::Error::Invalid("The selected workspace is now detached"));
        }
        let id = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .map(|group| group.issue)
            .max()
            .unwrap_or(100)
            .checked_add(1)
            .ok_or(cloud_runtime::Error::Invalid("Too many clouds"))?;
        pending.launch.revision = resolved.revision;
        let position = self
            .cloud_prototype
            .groups
            .next_position(&pending.workspace, &self.board);
        let mut group = CloudGroup::new(id, pending.title, pending.workspace, resolved.repository, position);
        group.environment.id.clone_from(&pending.launch.id);
        group.environment.connection = horizon_core::cloud_panel::CloudConnection::ManagedWorker;
        group.environment.provider = Some("runpod".into());
        group.environment.profile = Some(pending.launch.profile_name.clone());
        group.environment.image.clone_from(&pending.launch.profile.image);
        group.remote = Some(pending.launch);
        group.reconcile(&mut self.board);
        self.cloud_prototype.groups.0.push(group);
        self.cloud_prototype.production.creating = false;
        self.cloud_prototype.error = None;
        self.save_cloud_prototype();
        self.cloud_overview(ctx);
        if self.cloud_prototype.production.launch.workspace.is_some() {
            self.start_production_deployment(id, ctx);
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests;
