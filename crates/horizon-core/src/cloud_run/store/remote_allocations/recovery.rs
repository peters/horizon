//! Commit a non-creating observation only while both owned snapshots still match.

use super::{CloudWorkflowStore, Error, RemoteRuntimePhase, StoredRemoteAllocation, WorkspaceReplacement, binding};
use crate::cloud_run::interactive_worker::{
    InteractiveWorkerLifecycle, InteractiveWorkerRequest, InteractiveWorkerStatus,
};
use rusqlite::TransactionBehavior;

impl StoredRemoteAllocation {
    pub(crate) fn validate_worker_observation(
        &self,
        observation: Option<&InteractiveWorkerStatus>,
    ) -> Result<(), Error> {
        let request = self.worker_request()?;
        let runtime = self.workspace.state().runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        if let Some(status) = observation {
            validate_observation(status, &request)?;
            if runtime.worker.as_ref().is_some_and(|worker| worker != &status.worker)
                || runtime
                    .ssh
                    .as_ref()
                    .zip(status.ssh.as_ref())
                    .is_some_and(|(saved, observed)| saved != observed)
            {
                return Err(Error::InvalidWorkerObservation);
            }
        }
        Ok(())
    }

    pub(crate) fn recovery_request(&self) -> Result<InteractiveWorkerRequest, Error> {
        let runtime = self.workspace.state().runtime.as_ref().ok_or(Error::UnboundRuntime)?;
        if runtime.cleanup.is_some()
            || runtime.phase.stop_requested_at_millis().is_some()
            || matches!(
                runtime.phase,
                RemoteRuntimePhase::Cancelling | RemoteRuntimePhase::Deleting
            )
        {
            return Err(Error::RuntimeRecoveryUnavailable);
        }
        self.worker_request()
    }
}

impl CloudWorkflowStore {
    /// No observation authorizes creation or task startup. Reconciling deliberately
    /// does not mark a workspace ready merely because its worker advertises SSH.
    pub(crate) fn record_remote_worker_recovery(
        &self,
        expected: &StoredRemoteAllocation,
        observation: Option<&InteractiveWorkerStatus>,
    ) -> Result<StoredRemoteAllocation, Error> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        super::super::database::ensure_current_schema(&transaction)?;
        let row = binding::load_workspace(&transaction, &expected.workspace.state().spec.workspace_local_id)?
            .ok_or(Error::UnboundRuntime)?;
        let current = binding::recover(&transaction, &row)?;
        if current != *expected {
            return Err(Error::SnapshotConflict);
        }
        current.recovery_request()?;
        current.validate_worker_observation(observation)?;
        let mut next = current.workspace.state().clone();
        let runtime = next.runtime.as_mut().ok_or(Error::UnboundRuntime)?;
        if let Some(status) = observation {
            runtime.worker = Some(status.worker.clone());
            if let Some(ssh) = &status.ssh {
                runtime.ssh = Some(ssh.clone());
            }
        }
        // Absence never clears saved identity, panel intent, checkpoints, or permits replacement.
        runtime.phase = RemoteRuntimePhase::Reconciling;
        if next == *current.workspace.state() {
            return Ok(current);
        }
        let workspace = WorkspaceReplacement::new(&current.workspace, &next)?.persist(&transaction)?;
        transaction.commit()?;
        Ok(StoredRemoteAllocation {
            workspace,
            workflow: current.workflow,
        })
    }
}

fn validate_observation(status: &InteractiveWorkerStatus, request: &InteractiveWorkerRequest) -> Result<(), Error> {
    let worker = &status.worker;
    if !request.is_valid_for(request.target.provider)
        || !worker.is_valid_for(request.target.provider)
        || worker.identity.workflow_id != request.workflow_id
        || worker.identity.job_id != request.job_id
        || worker.target != request.target
        || worker.ssh_public_key != request.ssh_public_key
        || status.ssh.as_ref().is_some_and(|ssh| !ssh.is_complete())
        || (status.lifecycle == InteractiveWorkerLifecycle::Ready
            && !status.is_ready_for(request, time::OffsetDateTime::now_utc()))
    {
        return Err(Error::InvalidWorkerObservation);
    }
    Ok(())
}
