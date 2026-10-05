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
    /// As reviewed, each at the commit the launch must pin.
    siblings: Vec<cloud_runtime::siblings::Sibling>,
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

impl super::Production {
    /// The same-worker siblings checked for this launch, once reviewed for exactly the
    /// prepared repository, revision and profile.
    fn chosen_siblings(&self) -> cloud_runtime::Result<Vec<cloud_runtime::siblings::Sibling>> {
        self.launch
            .siblings
            .launch_siblings(
                &self.repository,
                self.launch.revision.as_deref(),
                &self.selected_profile,
            )
            .map_err(cloud_runtime::Error::Invalid)
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
        runpod_usable(form, profile)?;
        let profile = launch_profile(profile, form.provider, form.size)?;
        let placement = form.placement.for_profile(profile.gpu);
        let repository = horizon_core::Config::expand_tilde(&form.repository);
        let revision = if let Some(revision) = &form.launch.revision {
            revision.clone()
        } else if form.revision.is_empty() {
            "HEAD".into()
        } else {
            form.revision.clone()
        };
        let siblings = form.chosen_siblings()?;
        let reviewed = siblings.clone();
        let busy = form.creation_busy.clone();
        if busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(cloud_runtime::Error::Invalid(
                "Previous repository check is still stopping; retry shortly",
            ));
        }
        let cloud_id = cloud_runtime::new_id();
        let tailnet = form.tailnet.clone();
        let tailnet_root = self.cloud_prototype.root.clone();
        let selection_cloud = cloud_id.clone();
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
                id: cloud_id,
                revision: String::new(),
                profile_name: form.selected_profile.clone(),
                profile,
                placement,
            },
            siblings,
        });
        self.cloud_prototype.error = None;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = resolve(&repository, &revision, &reviewed, &cancel).and_then(|resolved| {
                if let Some(tailnet) = tailnet {
                    let root = tailnet_root.ok_or(cloud_runtime::Error::Invalid("Missing cloud settings"))?;
                    cloud_runtime::tailnet::select(&root, &selection_cloud, Some(&tailnet))?;
                }
                Ok(resolved)
            });
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
            self.close_cloud_creation();
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
            // A sibling checkout may have moved since it was checked; read the choice again.
            self.cloud_prototype.production.launch.siblings.forget_review();
            self.cloud_prototype.error = Some(error.to_string());
        }
    }

    fn finish_cloud_creation(
        &mut self,
        ctx: &egui::Context,
        mut pending: Pending,
        resolved: Resolved,
    ) -> cloud_runtime::Result<()> {
        pending.launch.revision = resolved.revision;
        let siblings = pending
            .siblings
            .into_iter()
            .map(|sibling| cloud_runtime::siblings::Binding {
                alias: sibling.alias,
                local_repository: sibling.local_repository,
                // Deployment pins exactly the reviewed commit and refuses a checkout that
                // moved on, including on a retry.
                revision: Some(sibling.revision),
            })
            .collect();
        // Cleared first, so a failed save while adding the card stays visible.
        self.cloud_prototype.error = None;
        let id = self.add_cloud_group(
            pending.title,
            pending.workspace,
            resolved.repository,
            pending.launch,
            siblings,
        )?;
        self.cloud_prototype.production.creating = false;
        // What this dialog held (a clone, a token, a typed key, its checks) does not outlive it.
        self.cloud_prototype.production.source = super::creation::source::State::default();
        self.cloud_prototype.production.checks = super::creation::checks::State::default();
        // The next cloud authorizes its own siblings; a reopened form starts unchecked.
        self.cloud_prototype.production.launch.siblings = super::creation::siblings::State::default();
        self.cloud_overview(ctx);
        if self.cloud_prototype.production.launch.workspace.is_some() {
            self.start_production_deployment(id, ctx);
        }
        Ok(())
    }

    /// Adds and saves a new cloud panel in `workspace` without starting it, and returns
    /// its card ID.
    /// # Errors
    /// The workspace was removed or detached, or there are too many clouds.
    pub(super) fn add_cloud_group(
        &mut self,
        title: String,
        workspace: String,
        repository: PathBuf,
        launch: CloudLaunch,
        siblings: Vec<cloud_runtime::siblings::Binding>,
    ) -> cloud_runtime::Result<u32> {
        let workspace_id = self
            .board
            .workspace_id_by_local_id(&workspace)
            .ok_or(cloud_runtime::Error::Invalid("The selected workspace was removed"))?;
        if self.workspace_is_detached(workspace_id) {
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
        let position = self.cloud_prototype.groups.next_position(&workspace, &self.board);
        let mut group = CloudGroup::new(id, title, workspace, repository, position);
        group.environment.id.clone_from(&launch.id);
        group.environment.connection = horizon_core::cloud_panel::CloudConnection::ManagedWorker;
        group.environment.provider = Some(launch.profile.provider.clone());
        group.environment.profile = Some(launch.profile_name.clone());
        group.environment.image.clone_from(&launch.profile.image);
        group.remote = Some(launch);
        group.siblings = siblings;
        group.reconcile(&mut self.board);
        self.cloud_prototype.groups.0.push(group);
        self.save_cloud_prototype();
        Ok(id)
    }
}

/// Refuses a `RunPod` cloud before anything is recorded when this machine has no
/// `RunPod` key, or while the first fetch has not said yet; the deployment would
/// fail the same way.
fn runpod_usable(
    form: &super::Production,
    profile: &horizon_core::cloud_runtime::prices::Profile,
) -> cloud_runtime::Result<()> {
    if super::creation::provider::current(form.provider, profile) != &cloud_runtime::provider::RUNPOD {
        return Ok(());
    }
    if form.prices.runpod_unknown() {
        return Err(cloud_runtime::Error::Invalid(
            "Horizon has not confirmed this machine's RunPod key yet; wait a moment or choose Try again on the prices",
        ));
    }
    if !form.prices.runpod_bound() {
        return Err(cloud_runtime::Error::Invalid(
            cloud_runtime::settings::RUNPOD_KEY_MISSING,
        ));
    }
    Ok(())
}

/// Whether a queued submission for a `RunPod` cloud waits for `RunPod`'s first answer.
/// The price fetch starts only once the profile has loaded, so processing the
/// submission in that same frame would record a cloud this machine may have no key for.
pub(super) fn awaits_runpod(form: &super::Production) -> bool {
    form.profiles
        .as_ref()
        .and_then(|config| config.profiles.get(&form.selected_profile))
        .is_some_and(|profile| {
            super::creation::provider::current(form.provider, profile) == &cloud_runtime::provider::RUNPOD
                && form.prices.runpod_pending()
        })
}

/// `profile` on the chosen provider at the chosen size, as the new cloud records it.
/// Refused before anything is recorded while Horizon cannot create clouds there.
pub(super) fn launch_profile(
    profile: &horizon_core::cloud_runtime::prices::Profile,
    chosen: Option<&'static cloud_runtime::provider::Description>,
    size: Option<super::machine_size::Size>,
) -> cloud_runtime::Result<horizon_core::cloud_runtime::prices::Profile> {
    let provider = super::creation::provider::current(chosen, profile);
    if !provider.creatable {
        return Err(cloud_runtime::Error::Invalid(
            "Horizon cannot create clouds on this provider yet; choose another provider",
        ));
    }
    // A choice made for another profile, or before the profile changed, is checked again.
    if !provider.supports(profile) {
        return Err(cloud_runtime::Error::Invalid(
            "This profile cannot run on the chosen provider",
        ));
    }
    super::creation::provider::sized(provider, profile, size)
}

/// The committed primary revision to launch, once every reviewed sibling checkout is still
/// at the commit it was reviewed at.
fn resolve(
    repository: &std::path::Path,
    revision: &str,
    siblings: &[cloud_runtime::siblings::Sibling],
    cancel: &cloud_runtime::Cancellation,
) -> cloud_runtime::Result<Resolved> {
    cancel.check()?;
    let repository = repository.canonicalize()?;
    let runner = cloud_runtime::command::Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let revision = cloud_runtime::repository::resolve_with_runner(&repository, revision, &runner)?;
    for sibling in siblings {
        sibling.unmoved(&runner)?;
    }
    Ok(Resolved { repository, revision })
}

#[cfg(all(test, unix))]
mod tests;
