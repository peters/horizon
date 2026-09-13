use super::{Error, admission::Admitted};
use crate::cloud_run::{
    CloudWorkflowStore,
    azure::{AzureCliCredential, AzureClient, AzureDeploymentPlan, AzureProfile, AzureWorker, resource_group_name},
};

pub(super) fn validate(admitted: &Admitted, profile: &AzureProfile) -> Result<(), Error> {
    AzureDeploymentPlan::validate_target(profile, &admitted.request.target).map_err(|_| Error::InvalidBinding)?;
    if admitted.network.is_some()
        || !admitted
            .cpu
            .as_ref()
            .ok_or(Error::InvalidBinding)?
            .matches_profile(profile)
            .map_err(|_| Error::InvalidBinding)?
    {
        return Err(Error::InvalidBinding);
    }
    let worker = super::super::retained_worker(&admitted.allocation)?;
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
    .map_err(|_| Error::InvalidBinding)
}

pub(super) fn provider(store: &CloudWorkflowStore, profile: &AzureProfile) -> Result<AzureClient, Error> {
    let credential = AzureCliCredential::new(profile.subscription_id.clone()).map_err(|_| Error::InvalidBinding)?;
    AzureClient::new(profile.clone(), credential, store.clone()).map_err(|_| Error::InvalidBinding)
}
