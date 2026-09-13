//! Existing Azure worker admission; the shared Git and credential transports remain authoritative.

use super::{ConfiguredRemoteGitSetupError as Error, RemoteProviderConfig, RemoteWorkspaceRecoveryError};
use crate::cloud_run::{
    CloudWorkflowStore, StoredRemoteAllocation,
    azure::{AzureCliCredential, AzureClient, AzureDeploymentPlan, AzureProfile, AzureWorker, resource_group_name},
};

pub(super) fn profile<'a>(
    store: &CloudWorkflowStore,
    config: &'a RemoteProviderConfig,
    allocation: &StoredRemoteAllocation,
) -> Result<&'a AzureProfile, Error> {
    let target = &allocation.workspace().state().spec.target;
    let profile = config.azure_profile(&target.profile)?;
    AzureDeploymentPlan::validate_target(profile, target).map_err(|_| Error::InvalidBinding)?;
    let binding = store
        .load_remote_cpu_profile_binding(allocation)
        .map_err(RemoteWorkspaceRecoveryError::from)?
        .ok_or(Error::InvalidBinding)?;
    if !binding.matches_profile(profile).map_err(|_| Error::InvalidBinding)? {
        return Err(Error::StateChanged);
    }
    let worker = allocation
        .workspace()
        .state()
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.worker.as_ref())
        .ok_or(Error::InvalidBinding)?;
    AzureWorker {
        workflow_id: worker.identity.workflow_id,
        job_id: worker.identity.job_id,
        subscription_id: profile.subscription_id.clone(),
        resource_group: resource_group_name(worker.identity.workflow_id, worker.identity.job_id),
        group_id: worker.identity.resource_id.clone(),
        image: worker.target.image.clone(),
        lifetime: worker.lifetime.clone(),
    }
    .validate()
    .map_err(|_| Error::InvalidBinding)?;
    Ok(profile)
}

pub(super) fn client(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    allocation: &StoredRemoteAllocation,
) -> Result<AzureClient, Error> {
    let profile = profile(store, config, allocation)?;
    // Constructors are lazy; the caller has already recovered the exact private identity.
    // Existing inspection may attest a host key through ARM. The shared transport then
    // requires that endpoint to equal the saved pin; it never records a replacement pin.
    let credential = AzureCliCredential::new(profile.subscription_id.clone()).map_err(|_| Error::InvalidBinding)?;
    AzureClient::new(profile.clone(), credential, store.clone()).map_err(|_| Error::InvalidBinding)
}
