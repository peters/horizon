//! One explicit provider read at a time; cached presentation changes only on events.

use super::{Context, HorizonHome, RemoteEnvironmentSummary, WakeOnDrop};
use horizon_core::{
    cloud_run::{CloudWorkflowStore, interactive_worker::InteractiveWorkerLifecycle},
    remote_environment_observation::{
        ConfiguredObservationError, RemoteEnvironmentObservation, observe_configured_remote_environment,
    },
    remote_provider_config::RemoteProviderConfig,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct ObservationState {
    pub(super) last_success: Option<CachedObservation>,
    pub(super) failure: Option<String>,
    pending: Option<PendingObservation>,
    repaint_context: Option<Context>,
}

pub(super) struct CachedObservation {
    pub(super) lifecycle: &'static str,
    pub(super) checked_at: String,
    pub(super) resource_id: Option<String>,
}

struct PendingObservation {
    rx: Receiver<Result<RemoteEnvironmentObservation, ObservationError>>,
    expected: RemoteEnvironmentSummary,
    discard: bool,
}

#[derive(Debug)]
enum ObservationError {
    WorkerUnavailable,
    StorageUnavailable,
    SelectionChanged,
    Check(ConfiguredObservationError),
}

impl ObservationError {
    fn message(self) -> String {
        match self {
            Self::WorkerUnavailable => "The provider check failed to start or finish; you can retry now.".into(),
            Self::StorageUnavailable => {
                "The saved environment could not be safely read; refresh the saved page before retrying.".into()
            }
            Self::SelectionChanged => {
                "The provider result does not match the selected saved environment; refresh before retrying.".into()
            }
            Self::Check(error) => error.to_string(),
        }
    }
}

impl ObservationState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn pending_label(&self) -> &'static str {
        if self.pending.as_ref().is_some_and(|pending| pending.discard) {
            "Waiting for previous check to finish…"
        } else {
            "Checking provider…"
        }
    }

    pub(super) fn invalidate(&mut self) {
        let changed = self.last_success.is_some()
            || self.failure.is_some()
            || self.pending.as_ref().is_some_and(|pending| !pending.discard);
        self.last_success = None;
        self.failure = None;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
        // Config reload can invalidate after this frame's modal was painted.
        if changed && let Some(ctx) = &self.repaint_context {
            ctx.request_repaint();
        }
    }

    pub(super) fn start(
        &mut self,
        home: &HorizonHome,
        config: &RemoteProviderConfig,
        expected: &RemoteEnvironmentSummary,
        ctx: &Context,
    ) {
        if self.pending.is_some() {
            return;
        }
        self.repaint_context = Some(ctx.clone());
        let (tx, rx) = mpsc::sync_channel(1);
        let selected = expected.clone();
        let config = config.clone();
        let home = home.clone();
        let wake = WakeOnDrop(ctx.clone());
        let worker = std::thread::Builder::new()
            .name("remote-environment-observation".into())
            .spawn(move || {
                let _wake = wake;
                let result = check(&home, &config, &selected);
                let _ = tx.send(result);
            });
        self.failure = None;
        match worker {
            Ok(_) => {
                self.pending = Some(PendingObservation {
                    rx,
                    expected: expected.clone(),
                    discard: false,
                });
            }
            Err(_) => self.failure = Some(ObservationError::WorkerUnavailable.message()),
        }
    }

    pub(super) fn drain_result(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(ObservationError::WorkerUnavailable),
        };
        if pending.discard {
            return;
        }
        let result = result.and_then(|observation| {
            if observation.saved != pending.expected {
                return Err(ObservationError::SelectionChanged);
            }
            Ok(CachedObservation::new(observation))
        });
        match result {
            Ok(observation) => {
                self.last_success = Some(observation);
                self.failure = None;
            }
            Err(error) => self.failure = Some(error.message()),
        }
    }
}

impl CachedObservation {
    fn new(observation: RemoteEnvironmentObservation) -> Self {
        let checked_at = observation
            .observed_at_rfc3339()
            .unwrap_or_else(|| format!("{} Unix ms", observation.observed_at_millis));
        let lifecycle = match observation.worker.as_ref().map(|worker| worker.lifecycle) {
            None => "No exact worker found (saved identity retained)",
            Some(InteractiveWorkerLifecycle::Provisioning) => "Worker provisioning",
            Some(InteractiveWorkerLifecycle::Ready) => "Worker ready (not workspace readiness)",
            Some(InteractiveWorkerLifecycle::Stopped) => "Worker stopped",
            Some(InteractiveWorkerLifecycle::Failed) => "Worker failed",
            Some(InteractiveWorkerLifecycle::Deleting) => "Worker deleting",
            Some(InteractiveWorkerLifecycle::Unknown) => "Worker status unknown",
        };
        Self {
            lifecycle,
            checked_at,
            resource_id: observation.worker.map(|worker| worker.identity.resource_id),
        }
    }
}

fn check(
    home: &HorizonHome,
    config: &RemoteProviderConfig,
    expected: &RemoteEnvironmentSummary,
) -> Result<RemoteEnvironmentObservation, ObservationError> {
    let store = CloudWorkflowStore::open(home).map_err(|_| ObservationError::StorageUnavailable)?;
    observe_configured_remote_environment(&store, config, expected).map_err(ObservationError::Check)
}

#[cfg(test)]
mod tests;
