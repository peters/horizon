//! Explicit confirmation and one bounded background Stop; client closure is not cancellation.

mod paint;

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, WakeOnDrop};
use horizon_core::{
    cloud_run::{CloudProvider, CloudWorkflowStore},
    remote_provider_config::RemoteProviderConfig,
    remote_workspace::{
        RemoteRuntimePhase,
        stop::{ConfiguredStopError, stop_configured_remote_environment},
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
    rx: Receiver<Result<RemoteEnvironmentSummary, StopError>>,
    expected: RemoteEnvironmentSummary,
    discard: bool,
}

struct StopNotice {
    expected: RemoteEnvironmentSummary,
    message: String,
    succeeded: bool,
}

#[derive(Debug)]
enum StopError {
    WorkerUnavailable,
    StorageUnavailable,
    SelectionChanged,
    Stop(ConfiguredStopError),
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
        }
    }
}

impl StopState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
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
        let expected = confirmation.expected.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let home = home.clone();
        let wake = WakeOnDrop(ctx.clone());
        let worker = std::thread::Builder::new()
            .name("remote-environment-stop".into())
            .spawn(move || {
                let _wake = wake;
                let result = execute(&home, &confirmation.config, &confirmation.expected);
                let _ = tx.send(result);
            });
        match worker {
            Ok(_) => {
                self.pending = Some(PendingStop {
                    rx,
                    expected,
                    discard: false,
                });
            }
            Err(_) => self.notice = Some(StopNotice::new(expected, Err(StopError::WorkerUnavailable))),
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
            self.notice = Some(StopNotice::new(pending.expected, result));
        }
        true
    }
}

impl StopNotice {
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
        }
    }
}

fn supported(summary: &RemoteEnvironmentSummary) -> bool {
    summary.provider == CloudProvider::LocalDocker && summary.worker_identity.is_some()
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
