//! Azure assembly only: retain original profile provenance before key or provider work.
use super::{Error, RemoteProviderConfig, storage};
use crate::{
    cloud_run::{
        CloudProvider, CloudWorkflowStore, StoredRemoteAllocation, StoredRemoteWorkspace, WorkerTarget,
        azure::{AzureCliCredential, AzureClient, AzureDeploymentPlan, AzureProfile},
        interactive_worker::InteractiveWorkerProvider,
    },
    remote_ssh_identity::RemoteSshIdentityStore,
};

pub(super) fn profile<'a>(config: &'a RemoteProviderConfig, target: &WorkerTarget) -> Result<&'a AzureProfile, Error> {
    let profile = config
        .azure_profile(&target.profile)
        .map_err(|_| Error::InvalidProfile)?;
    AzureDeploymentPlan::validate_target(profile, target).map_err(|_| Error::InvalidProfile)?;
    Ok(profile)
}

fn client(profile: &AzureProfile, store: &CloudWorkflowStore) -> Result<AzureClient, Error> {
    // Both constructors are lazy. Actual token acquisition starts inside the provider's
    // bounded ARM transport, after the shared coordinator has reserved the client key.
    let credential = AzureCliCredential::new(profile.subscription_id.clone()).map_err(|_| Error::InvalidProfile)?;
    AzureClient::new(profile.clone(), credential, store.clone()).map_err(|_| Error::InvalidProfile)
}

pub(super) fn start(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    saved: &StoredRemoteWorkspace,
    retain_until_millis: i64,
) -> Result<StoredRemoteAllocation, Error> {
    start_with(
        store,
        identities,
        profile(config, &saved.state().spec.target)?,
        saved,
        retain_until_millis,
        client,
    )
}

fn start_with<P: InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    profile: &AzureProfile,
    saved: &StoredRemoteWorkspace,
    retain_until_millis: i64,
    factory: impl FnOnce(&AzureProfile, &CloudWorkflowStore) -> Result<P, Error>,
) -> Result<StoredRemoteAllocation, Error> {
    AzureDeploymentPlan::validate_target(profile, &saved.state().spec.target).map_err(|_| Error::InvalidProfile)?;
    let allocation = store
        .allocate_remote_runtime(saved, retain_until_millis)
        .map_err(storage)?;
    // Failure here retains an interrupted allocation, never a key, creation claim or
    // provider request. Do not use start_remote_workspace: it prepares a key immediately.
    store
        .record_remote_cpu_profile_binding(&allocation, profile)
        .map_err(storage)?;
    let provider = factory(profile, store)?;
    super::super::retry_remote_workspace_setup(store, identities, &provider, &allocation)
        .map_err(|_| Error::SetupUnconfirmed)
}

/// Missing provenance is distinguishable from corrupt or changed provenance, but
/// neither can authorize credential access, no-handle recovery or a backfill.
pub(super) fn binding_matches(
    store: &CloudWorkflowStore,
    config: &RemoteProviderConfig,
    allocation: &StoredRemoteAllocation,
) -> Result<bool, Error> {
    let target = &allocation.workspace().state().spec.target;
    if target.provider != CloudProvider::Azure {
        return Ok(true);
    }
    let profile = profile(config, target)?;
    let Some(binding) = store.load_remote_cpu_profile_binding(allocation).map_err(storage)? else {
        return Ok(false);
    };
    if !binding.matches_profile(profile).map_err(|_| Error::InvalidProfile)? {
        return Err(Error::ContextChanged);
    }
    Ok(true)
}

pub(super) fn recover(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    allocation: &StoredRemoteAllocation,
) -> Result<StoredRemoteAllocation, Error> {
    recover_with(store, identities, config, allocation, client)
}

fn recover_with<P: InteractiveWorkerProvider>(
    store: &CloudWorkflowStore,
    identities: &RemoteSshIdentityStore,
    config: &RemoteProviderConfig,
    allocation: &StoredRemoteAllocation,
    factory: impl FnOnce(&AzureProfile, &CloudWorkflowStore) -> Result<P, Error>,
) -> Result<StoredRemoteAllocation, Error> {
    if !binding_matches(store, config, allocation)? {
        return Err(Error::SetupUnconfirmed);
    }
    let provider = factory(profile(config, &allocation.workspace().state().spec.target)?, store)?;
    // Existing recovery recovers (never creates) the reserved key and uses inspect or
    // reconcile without ensure. Host-key attestation may run its fixed guest command.
    super::super::recover(store, identities, &provider, allocation).map_err(|_| Error::SetupUnconfirmed)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
