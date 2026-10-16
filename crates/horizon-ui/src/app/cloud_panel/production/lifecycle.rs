use super::{Confirmation, Event, HorizonApp, Runtime, Settings, Stage, Store, channel, cloud_runtime, deployment};
use std::path::Path;

/// What a provider check reports.
type RecoveryResult = cloud_runtime::Result<cloud_runtime::lifecycle::ReconciledDeployment>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Deploy,
    Resize(deployment::ResizeTarget),
    Stop,
    Resume,
    Delete,
    Desktop,
    Remove,
    RevokeBrowserstack,
    Reconcile,
    /// Opens the rebuild's confirmation, which reads what the latest commit changes.
    PreviewRebuild,
    Rebuild,
    ContinueRebuild,
    CancelRebuild,
    ShareLocalNetwork,
    StopSharingLocalNetwork,
    /// Opens Cloud settings on the image repository of the cloud's profile. `pull` tells
    /// that the registry refused the worker's pull, not a push.
    ContainerRegistry {
        pull: bool,
    },
}

mod power;
use power::power_result;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CleanupRetry {
    Recorded,
    Legacy,
}

impl CleanupRetry {
    fn remember(runtime: &Runtime) -> Option<Self> {
        if runtime.stage != Some(Stage::Deleted) || !super::cards::node_cleanup_pending(runtime) {
            return None;
        }
        Some(
            if runtime
                .state
                .as_ref()
                .is_none_or(|state| state.tailnet_enrollment.is_empty())
            {
                Self::Legacy
            } else {
                Self::Recorded
            },
        )
    }
}

impl Runtime {
    fn refresh_cleanup_retry(&mut self, state: &cloud_runtime::state::Deployment) {
        if state.stage != Stage::Deleted {
            self.cleanup_retry = None;
        } else if state.tailnet_enrollment.cleanup_pending() {
            self.cleanup_retry = Some(CleanupRetry::Recorded);
        } else if !state.tailnet_enrollment.is_empty() {
            self.cleanup_retry = None;
        }
    }

    pub(super) fn observe_cleanup_retry(&mut self, event: &Event) {
        if matches!(event, Event::Stage(stage, _) if *stage != Stage::Deleted)
            || matches!(
                event,
                Event::Deleted(_) | Event::Ready(..) | Event::Stopped(_) | Event::Resumed
            )
        {
            self.cleanup_retry = None;
        }
    }

    /// Takes the saved record a report carries. While an image is built, pushed or
    /// switched, the record still says Validate for a new deployment and Ready for a
    /// rebuild of a running cloud, so neither replaces Build locally, Push image or
    /// Replace image of the live attempt: a failure there stays on that step.
    pub(super) fn adopt_snapshot(&mut self, state: cloud_runtime::state::Deployment) {
        self.refresh_cleanup_retry(&state);
        let image_step = matches!(self.stage, Some(Stage::Build | Stage::Push | Stage::Replace));
        if !(image_step && matches!(state.stage, Stage::Validate | Stage::Ready)) {
            self.stage = Some(state.stage);
        }
        self.state = Some(state);
    }

    /// Takes a GitHub sign-in prompt or outcome. An automatic sign-in opens GitHub's
    /// authorize page in the person's browser, which returns to Horizon by itself.
    pub(super) fn adopt_github(&mut self, prompt: cloud_runtime::github::Prompt) {
        if let cloud_runtime::github::Prompt::Web { url } = &prompt
            && let Err(error) = horizon_core::open_url(url)
        {
            tracing::warn!(%error, "could not open the GitHub sign-in page");
        }
        match prompt {
            // The package sign-in uses the same one-time box as the publishing sign-in.
            cloud_runtime::github::Prompt::Publish { .. } | cloud_runtime::github::Prompt::Packages { .. } => {
                self.publish = Some(prompt);
            }
            cloud_runtime::github::Prompt::Published { .. } | cloud_runtime::github::Prompt::PackagesEnded { .. } => {
                self.publish = None;
            }
            prompt => self.github = Some(prompt),
        }
    }

    fn remember_legacy_cleanup(&mut self, error: &str) {
        if self.stage == Some(Stage::Deleted)
            && error == cloud_runtime::tailnet::receipt::ManualCleanupReason::DeviceIdentityUnavailable.message()
        {
            self.cleanup_retry = Some(CleanupRetry::Legacy);
        }
    }

    /// Shows the failure an operation reported. Whether it may be a stop Horizon did
    /// not make is decided now, while a resize that failed still shows as running.
    pub(super) fn show_failure(&mut self, error: String, at: std::time::Instant) {
        self.remember_legacy_cleanup(&error);
        if self.cleanup_retry.is_some() {
            self.progress.stage(Stage::Deleted, at);
            self.stage = Some(Stage::Deleted);
        }
        self.progress.finish(at);
        self.error = Some(error);
        self.failure_needs_check = self.failure_may_be_a_stop();
    }

    /// Whether the failure just reported can be a stop Horizon did not make: the
    /// connection to a ready worker was lost, or a reconnect failed while it looked
    /// for the bound worker, or found it recorded as stopped. A failure after its
    /// operation was cancelled, as by a stop, is not one.
    pub(super) fn failure_may_be_a_stop(&self) -> bool {
        // The attempt reached Ready; a failed rebuild that reloaded a ready record did not.
        let lost = self.stage == Some(Stage::Ready) && self.progress.ended_in(Stage::Ready).is_some();
        let reconnect = self.operation.is_none()
            && self.rebuild.is_none()
            && (matches!(self.stage, Some(Stage::Provision | Stage::Readiness))
                || self.state.as_ref().is_some_and(|state| state.stop_requested));
        (lost || reconnect)
            && !self.progress.is_deletion()
            // A resize still running, or one whose journal is left, owns its failure in
            // either order of its two reports.
            && !self.resize.busy()
            && self.resize.pending.is_none()
            && self.recovery_receiver.is_none()
            && !self
                .cancel
                .as_ref()
                .is_some_and(cloud_runtime::Cancellation::is_cancelled)
            && self
                .state
                .as_ref()
                .is_some_and(cloud_runtime::lifecycle::may_stop_outside)
    }

    /// Holds the failure just reported and asks the provider whether the worker stopped
    /// without Horizon.
    pub(super) fn start_failure_check(
        &mut self,
        state_root: std::path::PathBuf,
        settings_path: std::path::PathBuf,
        ctx: &egui::Context,
    ) {
        let tx = self.hold_failure_for_check();
        let ctx = ctx.clone();
        let fence = super::session_record::fence(&state_root);
        std::thread::spawn(move || {
            fence.wait();
            // Read here, not on the UI thread: a failed read shows the failure as it was.
            let result = Settings::load(&settings_path)
                .and_then(|settings| {
                    cloud_runtime::lifecycle::check_lost_worker(
                        &state_root,
                        &settings,
                        &cloud_runtime::Cancellation::default(),
                    )
                })
                .and_then(|checked| checked.ok_or(cloud_runtime::Error::Invalid("The worker was not checked")));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Holds the failure for the provider check whose result the returned sender
    /// carries. That check alone decides: the watch of the lost worker ends now, so no
    /// other check holds the cloud once the card shows the result.
    pub(super) fn hold_failure_for_check(&mut self) -> std::sync::mpsc::Sender<RecoveryResult> {
        if let Some(watch) = self.cancel.take() {
            watch.cancel();
        }
        let (tx, rx) = channel();
        self.unexplained_failure = self.error.take();
        self.recovery_receiver = Some(rx);
        tx
    }

    fn start_reconciliation(&mut self, state_root: std::path::PathBuf, settings: Settings, ctx: &egui::Context) {
        if self.receiver.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.error = None;
        self.recovery_receiver = Some(rx);
        let worker_id = self.recovery_worker_id.trim().to_owned();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = cloud_runtime::lifecycle::reconcile(
                &state_root,
                &settings,
                (!worker_id.is_empty()).then_some(worker_id.as_str()),
                &cloud_runtime::Cancellation::default(),
            );
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn poll_recovery(&mut self) {
        let Some(receiver) = &self.recovery_receiver else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(cloud_runtime::Error::Invalid(
                "Provider reconciliation ended without a result; the operation remains fenced",
            )),
        };
        self.recovery_receiver = None;
        if let Ok(recovered) = &result {
            self.refresh_cleanup_retry(&recovered.state);
        }
        // A check Horizon started for a failure: a confirmed stop replaces the
        // failure, and anything else shows the failure as it was.
        if let Some(failure) = self.unexplained_failure.take() {
            match result {
                Ok(recovered) if recovered.confirmed_stopped() => self.show_stopped_outside(recovered.state),
                Ok(recovered) => {
                    // The check may have recorded another outcome; the card shows it beside the failure.
                    if recovered.report.outcome.needs_attention() {
                        self.push_note(recovered.report.outcome.explanation().into());
                    }
                    self.stage = Some(recovered.state.stage);
                    self.state = Some(recovered.state);
                    self.error = Some(failure);
                }
                Err(_) => self.error = Some(failure),
            }
            return;
        }
        match result {
            Ok(recovered) => {
                // A stopped worker, including one an agent on it stopped, only needs Resume.
                let stopped = recovered.confirmed_stopped();
                let resume = match recovered.stopped() {
                    cloud_runtime::provider::StoppedCost::WorkerKept => "Resume starts the same worker again.",
                    cloud_runtime::provider::StoppedCost::ServerDeleted => {
                        "Resume creates a new server that attaches the same workspace volume."
                    }
                };
                self.stage = Some(recovered.state.stage);
                self.state = Some(recovered.state);
                if self
                    .state
                    .as_ref()
                    .is_some_and(|state| matches!(state.operation, cloud_runtime::CreateState::Bound { .. }))
                {
                    self.recovery_worker_id.clear();
                }
                self.state_unavailable = false;
                self.error = (recovered.report.outcome.needs_attention() && !stopped)
                    .then(|| recovered.report.outcome.explanation().into());
                self.push_note(if stopped {
                    format!("The provider confirmed this worker is stopped. {resume}")
                } else {
                    recovered.report.outcome.explanation().into()
                });
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Another operation owns the cloud's worker or record.
    pub(super) fn busy(&self) -> bool {
        self.remote_release.is_some()
            || self.resize.busy()
            || self.checking_provider()
            || self.preparation.is_some()
            || self.sessions.saving()
            || (self.receiver.is_some() && self.stage != Some(Stage::Ready))
    }

    /// A provider check holds, or may hold, the cloud: Check provider or the check of a
    /// failure.
    pub(super) fn checking_provider(&self) -> bool {
        self.recovery_receiver.is_some()
    }

    fn start_device_release(&mut self, state_root: std::path::PathBuf, settings: Settings, ctx: &egui::Context) {
        if !self.can_release_remote_devices() {
            return;
        }
        let (tx, rx) = channel();
        self.remote_release_error = None;
        self.remote_release = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = cloud_runtime::lifecycle::revoke_browserstack(
                &state_root,
                &settings,
                &cloud_runtime::Cancellation::default(),
            );
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    pub(super) fn can_release_remote_devices(&self) -> bool {
        self.remote_release.is_none()
            // The release locks the record, which a preparation or a session save holds.
            && self.preparation.is_none()
            && !self.sessions.saving()
            && (self.receiver.is_none() || self.stage == Some(Stage::Ready))
            && self.state.as_ref().is_some_and(|state| {
                state.requires_browserstack_release()
                    && matches!(state.operation, cloud_runtime::CreateState::Bound { .. })
                    // The release acts on the worker as recorded, so it waits for a pending replacement.
                    && state.image_replacement.is_none()
                    && state.stage != Stage::Replace
            })
    }

    pub(super) fn poll_remote_release(&mut self) {
        if self.state.as_ref().is_some_and(|state| state.browserstack_released) {
            self.remote_release_error = None;
        }
        let Some(receiver) = &self.remote_release else { return };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(cloud_runtime::Error::Invalid(
                "Remote device release ended without a result",
            )),
        };
        self.remote_release = None;
        match result {
            Ok(state) => {
                self.remote_release_error = None;
                self.state = Some(state);
                self.push_note("Remote devices released and copied credentials removed".into());
            }
            Err(error) => self.remote_release_error = Some(error.to_string()),
        }
    }
}

fn action_settings(runtime: &Runtime, action: Action, path: &Path) -> cloud_runtime::Result<Option<Settings>> {
    // Cleanup retry has no live worker watch to retain during its settings preflight.
    if action == Action::Delete && runtime.stage == Some(Stage::Deleted) {
        return Ok(None);
    }
    runtime
        .agent_stop
        .as_ref()
        .filter(|_| action == Action::Stop)
        .map(|order| order.settings.clone())
        .map_or_else(|| Settings::load(path), Ok)
        .map(Some)
}

fn checked_settings(
    settings: Option<Settings>,
    path: &Path,
    cancel: &cloud_runtime::Cancellation,
) -> cloud_runtime::Result<Settings> {
    cancel.check()?;
    let settings = settings.map_or_else(|| Settings::load(path), Ok)?;
    cancel.check()?;
    Ok(settings)
}

/// A worker operation that failed its preflight is still that operation's failure: it
/// gets its own attempt, so the status names it and diagnosis ignores earlier output.
fn fail_before_start(runtime: &mut Runtime, action: Action, error: String) {
    if action == Action::RevokeBrowserstack {
        // Reported as the device release it was, which Manage finishes; not as a deployment.
        runtime.remote_release_error = Some(error);
        return;
    }
    if matches!(action, Action::Stop | Action::Resume | Action::Delete) && !runtime.connected_ready() {
        begin_operation(runtime, action);
    }
    if runtime.cleanup_retry.is_some() {
        runtime.show_failure(error, std::time::Instant::now());
    } else {
        runtime.error = Some(error);
    }
}

/// Resets what the card shows for a newly started worker operation.
fn begin_operation(runtime: &mut Runtime, action: Action) {
    runtime.cleanup_retry = if action == Action::Delete {
        CleanupRetry::remember(runtime)
    } else {
        None
    };
    if action == Action::Delete {
        // Until core reports its first step, the step it will start with stands
        // in, so Cancel shows only when that step can still be cancelled.
        runtime.progress.begin_deletion();
        runtime.operation = Some(action);
        // Both error channels the card shows belong to earlier attempts.
        runtime.error = None;
        runtime.remote_release_error = None;
        runtime.stage = Some(first_deletion_step(runtime.state.as_ref()));
    } else {
        // Every operation times and diagnoses its own output: a failed deletion's
        // frozen steps and an earlier deployment's clock do not carry over.
        runtime.progress.reset();
        runtime.operation = Some(action);
        // A stop reports as stopping from its start; its failure is a stop's, not a provision's.
        runtime.stage = Some(if action == Action::Stop {
            Stage::Stopping
        } else {
            Stage::Provision
        });
    }
}

/// The deletion step core starts with for this record. Only hosted-device release
/// for a requested worker can still be cancelled; every other deletion starts with
/// a request that cannot be recalled.
fn first_deletion_step(state: Option<&cloud_runtime::state::Deployment>) -> Stage {
    match state {
        Some(state) if state.operation == cloud_runtime::CreateState::Prepared => Stage::DeleteStorage,
        Some(state) if state.requires_browserstack_release() => Stage::ReleaseDevices,
        _ => Stage::DeleteWorker,
    }
}

impl HorizonApp {
    /// Asks the provider, before the card shows the failure of cloud `id`, whether it
    /// is a stop Horizon did not make. The check shows as Checking provider; it reads
    /// the settings off the UI thread, and without them the failure shows as it was.
    pub(super) fn check_failure_with_provider(&mut self, id: u32, ctx: &egui::Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let Some(launch) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| group.remote.as_ref())
        else {
            return;
        };
        let Ok(state_root) = cloud_runtime::state::cloud_directory(&root, &launch.id) else {
            return;
        };
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
            runtime.start_failure_check(state_root, root.join("settings.json"), ctx);
        }
    }

    pub(super) fn change_production_worker(&mut self, id: u32, action: Action, ctx: &egui::Context) {
        if let Action::Resize(target) = action {
            self.start_production_resize(id, target, ctx);
            return;
        }
        if self.rebuild_action(id, action, ctx) {
            return;
        }
        let Some(launch) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| group.remote.clone())
        else {
            return;
        };
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let runtime = self.cloud_prototype.production.runtimes.entry(id).or_default();
        if runtime.busy() {
            return;
        }
        // An operation chosen now, as a person's Stop, ends the claim of an earlier agent
        // Stop on its request, which a failed Stop keeps while its outcome may be
        // uncertain; this holds even when the choice is refused below. The Stop for an
        // agent's request sets the claim again once it started.
        runtime.stopping_for = None;
        let settings_path = root.join("settings.json");
        let settings = match action_settings(runtime, action, &settings_path) {
            Ok(settings) => settings,
            Err(error) => {
                fail_before_start(runtime, action, error.to_string());
                return;
            }
        };
        let state_root = match cloud_runtime::state::cloud_directory(&root, &launch.id) {
            Ok(path) => path,
            Err(error) => {
                fail_before_start(runtime, action, error.to_string());
                runtime.state_unavailable = true;
                return;
            }
        };
        if action == Action::Reconcile
            && let Some(settings) = settings
        {
            runtime.start_reconciliation(state_root, settings, ctx);
            return;
        }
        if action == Action::RevokeBrowserstack
            && let Some(settings) = settings
        {
            runtime.start_device_release(state_root, settings, ctx);
            return;
        }
        if let Some(cancel) = runtime.cancel.take() {
            cancel.cancel();
        }
        runtime.desktop = None;
        runtime.confirmation = Confirmation::None;
        runtime.rebuild = None;
        let (tx, rx) = channel();
        runtime.receiver = Some(rx);
        runtime.sender = Some(tx.clone());
        begin_operation(runtime, action);
        // A Stop for an agent's request carries it out and names the agent.
        let agent_stop = runtime
            .agent_stop
            .take()
            .filter(|_| action == Action::Stop)
            .map(|order| order.stop);
        if let Some(stop) = &agent_stop {
            runtime.push_log(format!("{} asked to stop this cloud: {}", stop.agent, stop.reason));
        }
        let cancel = cloud_runtime::Cancellation::default();
        runtime.cancel = Some(cancel.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let root = state_root;
            let emit = |event| {
                let _ = tx.send(event);
                ctx.request_repaint();
            };
            let settings = match checked_settings(settings, &settings_path, &cancel) {
                Ok(settings) => settings,
                Err(error) => {
                    emit(Event::failed(error.to_string()));
                    return;
                }
            };
            let Some(result) = power_result(action, agent_stop.as_ref(), &root, &settings, &cancel, &emit) else {
                return;
            };
            if let Ok(store) = Store::lock(&root)
                && let Ok(Some(state)) = store.load()
            {
                let _ = tx.send(Event::Snapshot(Box::new(state)));
            }
            let _ = tx.send(result.unwrap_or_else(|error| Event::failed(error.to_string())));
            ctx.request_repaint();
        });
    }

    /// Starts a fresh background eligibility check; completion applies the removal.
    pub(super) fn remove_deleted_cloud(&mut self, id: u32, ctx: &egui::Context) -> super::close::RemovalStart {
        self.begin_cloud_removal(id, super::close::Origin::Direct, ctx)
    }

    pub(super) fn cloud_removal_error(&mut self, id: u32, message: String) {
        if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
            runtime.remember_legacy_cleanup(&message);
            runtime.error = Some(message.clone());
        }
        self.cloud_prototype.error = Some(message);
    }
}
