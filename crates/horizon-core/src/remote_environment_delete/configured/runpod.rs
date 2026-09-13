use super::{Error, admission::Admitted};
use crate::cloud_run::{
    CloudWorkflowStore,
    runpod::{RunPodApiKey, RunPodClient, RunPodDeletionWorkerProvider, RunPodProfile, validate_target},
};

pub(super) fn validate(admitted: &Admitted, profile: &RunPodProfile) -> Result<(), Error> {
    validate_target(&admitted.request.target, profile).map_err(|_| Error::InvalidBinding)?;
    let network = admitted.network.as_ref().ok_or(Error::InvalidBinding)?;
    network.validate().map_err(|_| Error::InvalidBinding)?;
    let worker = super::super::retained_worker(&admitted.allocation)?;
    let id = &worker.identity.resource_id;
    if admitted.cpu.is_some()
        || id.len() > 191
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || profile
            .data_center_id
            .as_ref()
            .is_some_and(|id| id != &network.data_center_id)
    {
        return Err(Error::InvalidBinding);
    }
    Ok(())
}

pub(super) fn provider(
    store: &CloudWorkflowStore,
    profile: &RunPodProfile,
    admitted: &Admitted,
) -> Result<RunPodDeletionWorkerProvider, Error> {
    let key = RunPodApiKey::from_env().map_err(|_| Error::CredentialUnavailable)?;
    RunPodDeletionWorkerProvider::new_with_network_volume(
        RunPodClient::new(&key, store.clone()),
        profile.clone(),
        &admitted.request,
        admitted.network.as_ref().ok_or(Error::InvalidBinding)?,
    )
    .map_err(|_| Error::InvalidBinding)
}
