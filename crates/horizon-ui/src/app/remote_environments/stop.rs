//! One explicit background Stop or saved-Stop check; client closure never replays either.

mod paint;

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, WakeOnDrop};
use horizon_core::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, WorkerLifetime, interactive_worker_stop::InteractiveWorkerStopObservation,
    },
    remote_provider_config::RemoteProviderConfig,
    remote_workspace::{
        RemoteRuntimePhase,
        stop::{
            ConfiguredStopConfirmation, ConfiguredStopConfirmationError, ConfiguredStopError,
            confirm_configured_remote_environment_stop, stop_configured_remote_environment,
        },
    },
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct StopState {
    confirmation: Option<Confirmation>,
    pending: Option<PendingStop>,
    notice: Option<StopNotice>,
    repaint_context: Option<Context>,
}

struct Confirmation {
    expected: RemoteEnvironmentSummary,
    config: RemoteProviderConfig,
}

struct PendingStop {
    rx: Receiver<Result<StopResult, StopError>>,
    expected: RemoteEnvironmentSummary,
    operation: Operation,
    discard: bool,
}

#[derive(Clone, Copy)]
enum Operation {
    Stop,
    Check,
}

enum StopResult {
    Stopped(RemoteEnvironmentSummary),
    Checked(ConfiguredStopConfirmation),
}

struct StopNotice {
    expected: RemoteEnvironmentSummary,
    message: String,
    succeeded: bool,
    checked: bool,
}

#[derive(Debug)]
enum StopError {
    WorkerUnavailable,
    StorageUnavailable,
    SelectionChanged,
    Stop(ConfiguredStopError),
    Check(ConfiguredStopConfirmationError),
}

impl StopError {
    fn message(self) -> String {
        match self {
            Self::WorkerUnavailable => {
                "Stop could not finish. Refresh saved inventory before explicitly retrying.".into()
            }
            Self::StorageUnavailable => "The saved environment could not be safely accessed for Stop.".into(),
            Self::SelectionChanged => {
                "The Stop result does not match this environment. Refresh before retrying.".into()
            }
            Self::Stop(error) => error.to_string(),
            Self::Check(error) => error.to_string(),
        }
    }
}

impl StopState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn pending_label(&self) -> &'static str {
        match self.pending.as_ref().map(|pending| pending.operation) {
            Some(Operation::Check) => {
                "Checking saved Stop. No Stop request is sent; verified completion may update this saved record."
            }
            _ => "An explicitly confirmed Stop is pending. Closing this overview does not cancel it.",
        }
    }

    pub(super) fn cancel_confirmation(&mut self) {
        if self.confirmation.take().is_some() {
            self.wake();
        }
    }

    pub(super) fn invalidate(&mut self) {
        let had_confirmation = self.confirmation.take().is_some();
        let had_notice = self.notice.take().is_some();
        let pending_changed = self.pending.as_ref().is_some_and(|pending| !pending.discard);
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
        if had_confirmation || had_notice || pending_changed {
            self.wake();
        }
    }

    fn wake(&self) {
        if let Some(ctx) = &self.repaint_context {
            ctx.request_repaint();
        }
    }

    pub(super) fn prepare(
        &mut self,
        expected: &RemoteEnvironmentSummary,
        config: &RemoteProviderConfig,
        ctx: &Context,
    ) {
        if self.is_pending() || !supported(expected) {
            return;
        }
        self.repaint_context = Some(ctx.clone());
        self.confirmation = Some(Confirmation {
            expected: expected.clone(),
            config: config.clone(),
        });
        self.notice = None;
        ctx.request_repaint();
    }

    pub(super) fn start(
        &mut self,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: &RemoteEnvironmentSummary,
        ctx: &Context,
    ) -> bool {
        if self.is_pending() {
            return false;
        }
        let Some(confirmation) = self.confirmation.take() else {
            return false;
        };
        if confirmation.expected != *selected || confirmation.config != *config {
            self.wake();
            return false;
        }
        self.spawn(home, confirmation.config, confirmation.expected, Operation::Stop, ctx)
    }

    pub(super) fn check(
        &mut self,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        selected: &RemoteEnvironmentSummary,
        ctx: &Context,
    ) -> bool {
        if self.is_pending() || !check_supported(selected) {
            return false;
        }
        self.confirmation = None;
        self.notice = None;
        self.spawn(home, config.clone(), selected.clone(), Operation::Check, ctx)
    }

    fn spawn(
        &mut self,
        home: &HorizonHome,
        config: RemoteProviderConfig,
        expected: RemoteEnvironmentSummary,
        operation: Operation,
        ctx: &Context,
    ) -> bool {
        let (tx, rx) = mpsc::sync_channel(1);
        let home = home.clone();
        let selected = expected.clone();
        let wake = WakeOnDrop(ctx.clone());
        self.repaint_context = Some(ctx.clone());
        let worker = std::thread::Builder::new()
            .name(
                match operation {
                    Operation::Stop => "remote-environment-stop",
                    Operation::Check => "remote-environment-stop-check",
                }
                .into(),
            )
            .spawn(move || {
                let _wake = wake;
                let result = match operation {
                    Operation::Stop => execute(&home, &config, &selected).map(StopResult::Stopped),
                    Operation::Check => execute_check(&home, &config, &selected).map(StopResult::Checked),
                };
                let _ = tx.send(result);
            });
        match worker {
            Ok(_) => {
                self.pending = Some(PendingStop {
                    rx,
                    expected,
                    operation,
                    discard: false,
                });
            }
            Err(_) => {
                self.notice = Some(StopNotice::finish(
                    expected,
                    operation,
                    Err(StopError::WorkerUnavailable),
                ));
            }
        }
        ctx.request_repaint();
        self.is_pending()
    }

    pub(super) fn drain_result(&mut self) -> bool {
        let Some(pending) = self.pending.take() else {
            return false;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return false;
            }
            Err(TryRecvError::Disconnected) => Err(StopError::WorkerUnavailable),
        };
        if !pending.discard {
            self.notice = Some(StopNotice::finish(pending.expected, pending.operation, result));
        }
        true
    }
}

impl StopNotice {
    fn finish(expected: RemoteEnvironmentSummary, operation: Operation, result: Result<StopResult, StopError>) -> Self {
        match operation {
            Operation::Stop => Self::new(
                expected,
                result.and_then(|result| match result {
                    StopResult::Stopped(saved) => Ok(saved),
                    StopResult::Checked(_) => Err(StopError::SelectionChanged),
                }),
            ),
            Operation::Check => Self::checked(
                expected,
                result.and_then(|result| match result {
                    StopResult::Checked(checked) => Ok(checked),
                    StopResult::Stopped(_) => Err(StopError::SelectionChanged),
                }),
            ),
        }
    }

    fn new(expected: RemoteEnvironmentSummary, result: Result<RemoteEnvironmentSummary, StopError>) -> Self {
        let result = result.and_then(|saved| {
            if !same_target(&expected, &saved)
                || saved.revision < expected.revision
                || !matches!(saved.saved_phase, Some(RemoteRuntimePhase::Stopped { .. }))
            {
                return Err(StopError::SelectionChanged);
            }
            Ok(())
        });
        let succeeded = result.is_ok();
        let message = match result {
            Ok(()) => "Stop verified and saved. This is not a live provider check.".into(),
            Err(error) => error.message(),
        };
        Self {
            expected,
            message,
            succeeded,
            checked: false,
        }
    }

    fn checked(expected: RemoteEnvironmentSummary, result: Result<ConfiguredStopConfirmation, StopError>) -> Self {
        use InteractiveWorkerStopObservation::{Absent, Pending, RetainedStopped};
        let result = result.and_then(|result| {
            if !valid_check_result(&expected, &result) {
                return Err(StopError::SelectionChanged);
            }
            Ok(result.observation)
        });
        let succeeded = matches!(result, Ok(RetainedStopped));
        let message = match result {
            Ok(RetainedStopped) => {
                "Retained Stop confirmed at this check; completion is saved. No Stop request was sent.".into()
            }
            Ok(Pending) => {
                "Stop is not yet confirmed. Saved intent and identity are unchanged; no Stop request was sent.".into()
            }
            Ok(Absent) => {
                "Worker is absent: retained Stop cannot be certified. Saved intent and identity are unchanged.".into()
            }
            Err(StopError::WorkerUnavailable) => {
                "The Stop check could not finish. Check again does not resend Stop.".into()
            }
            Err(error) => error.message(),
        };
        Self {
            expected,
            message,
            succeeded,
            checked: true,
        }
    }
}

fn valid_check_result(expected: &RemoteEnvironmentSummary, result: &ConfiguredStopConfirmation) -> bool {
    if !check_supported(expected) {
        return false;
    }
    let mut allowed = expected.clone();
    if result.observation == InteractiveWorkerStopObservation::RetainedStopped
        && let Some(RemoteRuntimePhase::Stopping { requested_at_millis }) = expected.saved_phase
    {
        let Some(RemoteRuntimePhase::Stopped {
            requested_at_millis: saved_request,
            observed_at_millis,
        }) = result.saved.saved_phase
        else {
            return false;
        };
        let Some(revision) = expected.revision.checked_add(1) else {
            return false;
        };
        if saved_request != requested_at_millis || observed_at_millis < requested_at_millis {
            return false;
        }
        allowed.revision = revision;
        allowed.saved_phase = result.saved.saved_phase;
    }
    result.saved == allowed
}

fn check_supported(summary: &RemoteEnvironmentSummary) -> bool {
    cfg!(target_os = "linux")
        && summary.provider == CloudProvider::RunPod
        && summary.lifetime == WorkerLifetime::Persistent
        && summary.worker_identity.is_some()
        && matches!(
            summary.saved_phase,
            Some(RemoteRuntimePhase::Stopping { .. } | RemoteRuntimePhase::Stopped { .. })
        )
}

fn supported(summary: &RemoteEnvironmentSummary) -> bool {
    summary.provider == CloudProvider::LocalDocker
        && summary.lifetime == WorkerLifetime::Persistent
        && summary.worker_identity.is_some()
}

fn same_target(left: &RemoteEnvironmentSummary, right: &RemoteEnvironmentSummary) -> bool {
    left.workspace_local_id == right.workspace_local_id
        && left.owning_session_id == right.owning_session_id
        && left.provider == right.provider
        && left.profile == right.profile
        && left.generation == right.generation
        && left.worker_identity == right.worker_identity
}

fn execute(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentSummary, StopError> {
    let store = CloudWorkflowStore::open(home).map_err(|_| StopError::StorageUnavailable)?;
    stop_configured_remote_environment(&store, config, expected).map_err(StopError::Stop)
}

fn execute_check(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<ConfiguredStopConfirmation, StopError> {
    if !check_supported(expected) {
        return Err(StopError::Check(ConfiguredStopConfirmationError::UnsupportedProvider));
    }
    // A check must not initialize an absent/corrupt store. Completion intentionally needs a writer.
    let reader = CloudWorkflowStore::open_read_only(home).map_err(|_| StopError::StorageUnavailable)?;
    let store = CloudWorkflowStore::open_path(reader.path()).map_err(|_| StopError::StorageUnavailable)?;
    confirm_configured_remote_environment_stop(&store, config, expected).map_err(StopError::Check)
}

pub(super) fn show(
    ui: &mut egui::Ui,
    state: &StopState,
    selected: &RemoteEnvironmentSummary,
    idle: bool,
    action: &mut InventoryAction,
) {
    paint::show(ui, state, selected, idle, action);
}

#[cfg(test)]
mod tests;
