//! Each provider's lifecycle behind one interface. `lifecycle` asks the cloud's
//! provider for what it records and does, and keeps what every provider shares: the
//! deployment record, its lock and what a found or terminated worker means for it.
//! Adding a provider means one implementation here and its description in
//! `horizon_cloud::provider`.
mod azure;
#[cfg(feature = "tailnet-node-cleanup")]
mod deletion;
mod docker_host;
mod hetzner;
mod runpod;

use super::{
    Cancellation, CreateState, Event, Result,
    mutation::Observer,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::runpod::recovery::Reconciliation;
use horizon_cloud::{Profile, provider::Kind};

pub(super) use azure::REFUSED as AZURE_REFUSED;
#[cfg(feature = "tailnet-node-cleanup")]
pub(super) use deletion::confirmed as deletion_confirmed;

/// What a provider does for a cloud's lifecycle.
pub(super) trait Lifecycle {
    /// Checks the recorded worker without creating, starting or deleting anything,
    /// recording what it finds; returns the report and the server fence after it.
    fn check(
        &self,
        store: &Store,
        state: &mut Deployment,
        worker_hint: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)>;
    /// Stops the cloud's worker as the provider stops it (`provider::StoppedCost`),
    /// telling `observe` just before each provider mutation and once it is settled.
    fn stop(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, observe: Observer<'_>) -> Result<()>;
    /// Resumes a stopped cloud; the reconnect that follows finishes it. `observe` is
    /// told as for [`Lifecycle::stop`].
    fn resume(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, observe: Observer<'_>)
    -> Result<()>;
    /// Releases the worker's hosted devices and removes their credentials from it.
    fn release_devices(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()>;
    /// Deletes everything the cloud created on the provider, each proven absent, and
    /// emits each step; recording the cloud as deleted is left to the caller.
    fn delete(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<()>;
}

/// Which provider `profile` names; a match on it must handle every provider.
fn kind(profile: &Profile) -> Kind {
    horizon_cloud::provider::Description::of(profile).kind
}

/// Checks what the provider needs before any record, build or request, so an
/// unsupported request fails at once and leaves nothing behind.
pub(super) fn preflight(cloud_id: &str, profile: &Profile, settings: &Settings) -> Result<()> {
    match kind(profile) {
        Kind::Hetzner => super::deployment::hetzner::preflight(cloud_id, profile, settings),
        Kind::DockerHost => docker_host::preflight(profile, settings),
        Kind::RunPod => Ok(()),
        // The gate for every caller: a deployment record is written only after this.
        Kind::Azure => Err(azure::NOT_CREATABLE),
    }
}

/// Checks what the provider needs before the deployment is recorded for `image`,
/// unless its worker is already requested or bound and so only reconciled.
pub(super) fn admit(settings: &Settings, profile: &Profile, image: &str, operation: &CreateState) -> Result<()> {
    match kind(profile) {
        Kind::Hetzner => super::deployment::hetzner::admit(settings, image, operation),
        // Never reached: `preflight` and `compute` refuse an Azure cloud first.
        Kind::DockerHost | Kind::RunPod | Kind::Azure => Ok(()),
    }
}

/// The machine types a worker may run on, in the order they are tried: `RunPod` CPU
/// flavors, a Hetzner cloud's server types, an Azure cloud's catalog VM sizes, or a
/// Docker host cloud's saved hosts.
pub(super) fn cpu_flavors(profile: &Profile, settings: &Settings) -> Result<Vec<String>> {
    match kind(profile) {
        Kind::Hetzner => hetzner::cpu_flavors(settings),
        Kind::DockerHost => docker_host::hosts(settings),
        Kind::RunPod => runpod::cpu_flavors(profile, settings),
        Kind::Azure => azure::cpu_flavors(settings),
    }
}

/// Where a worker may run: `RunPod` data centers, a Hetzner cloud's locations, an
/// Azure cloud's catalog regions, or a Docker host cloud's saved hosts.
pub(super) fn data_centers(profile: &Profile, settings: &Settings) -> Result<Vec<String>> {
    match kind(profile) {
        Kind::Hetzner => hetzner::data_centers(settings),
        Kind::DockerHost => docker_host::hosts(settings),
        Kind::RunPod => Ok(settings.data_centers.clone()),
        Kind::Azure => azure::data_centers(settings),
    }
}

/// The provider a cloud of `request` deploys on, with its client.
/// # Errors
/// Refuses missing credentials and settings the provider needs.
pub(super) fn compute(request: &super::deployment::Request) -> Result<Box<dyn super::deployment::Compute + '_>> {
    Ok(match kind(&request.profile) {
        Kind::Hetzner => Box::new(super::deployment::hetzner::Compute::new(&request.settings)?),
        Kind::DockerHost => Box::new(docker_host::DockerHost {
            settings: &request.settings,
        }),
        Kind::RunPod => Box::new(horizon_cloud::runpod::RunPod::new(request.settings.credential()?)),
        Kind::Azure => return Err(azure::NO_CLIENT),
    })
}

/// Whether anything the cloud created on its provider may still exist, read from
/// local records only, so it needs no provider credential.
pub(super) fn retained(store: &Store, state: &Deployment) -> Result<bool> {
    match kind(&state.profile) {
        Kind::Hetzner => hetzner::retained(store),
        Kind::DockerHost => docker_host::retained(store),
        Kind::RunPod => runpod::retained(store, state),
        // Nothing creates Azure resources yet, so a record cannot hold any.
        Kind::Azure => Ok(false),
    }
}

/// The lifecycle of the provider `state`'s profile names.
pub(super) fn lifecycle<'a>(state: &Deployment, settings: &'a Settings) -> Box<dyn Lifecycle + 'a> {
    match kind(&state.profile) {
        Kind::Hetzner => Box::new(hetzner::Hetzner { settings }),
        Kind::DockerHost => Box::new(docker_host::DockerHost { settings }),
        Kind::RunPod => Box::new(runpod::RunPod { settings }),
        Kind::Azure => Box::new(azure::Azure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn azure() -> Profile {
        serde_json::from_value(
            serde_json::json!({"provider":"azure","image":"registry.example/worker","cpu":4,"memory_gb":8}),
        )
        .unwrap()
    }

    fn settings() -> Settings {
        serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused",
            "registry_pull_auth_id":null,"cpu_flavors":["cpu3c"],"gpu_types":[],"data_centers":["EU-RO-1"]
        }))
        .unwrap()
    }

    #[test]
    fn an_azure_cloud_is_refused_before_anything_is_recorded_or_requested() {
        let (profile, settings) = (azure(), settings());
        assert_eq!(kind(&profile), Kind::Azure);
        let refusal = preflight("azure-fixture", &profile, &settings).unwrap_err().to_string();
        assert_eq!(refusal, "Horizon cannot create clouds on Azure yet");
        let request = super::super::deployment::Request::new(
            "azure-fixture".into(),
            "/synthetic/not-mounted".into(),
            "a".repeat(40),
            profile.clone(),
            "/synthetic/state".into(),
            settings.clone(),
        );
        let client = compute(&request).err().map(|error| error.to_string());
        assert_eq!(client.as_deref(), Some("Horizon has no Azure deployment client yet"));
        assert_ne!(
            client.as_deref(),
            Some(refusal.as_str()),
            "not read as the profile refusal"
        );
        // The RunPod settings in the fixture must not answer for an Azure cloud.
        assert!(cpu_flavors(&profile, &settings).is_err());
        assert!(data_centers(&profile, &settings).is_err());
    }

    #[test]
    fn configured_azure_answers_from_the_catalog_and_new_clouds_stay_refused() {
        let temp = tempfile::tempdir().unwrap();
        let token = temp.path().join("arm-token");
        std::fs::write(&token, "synthetic-arm-token").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut settings = settings();
        settings.azure = Some(
            serde_json::from_value(serde_json::json!({
                "token_file": token, "subscription_id": "00000000-0000-4000-8000-000000000001",
                "vm_sizes": ["Standard_D4s_v3", "Standard_F8s_v2"], "regions": ["northeurope", "swedencentral"]
            }))
            .unwrap(),
        );
        let profile = azure();
        assert_eq!(
            cpu_flavors(&profile, &settings).unwrap(),
            ["Standard_D4s_v3", "Standard_F8s_v2"]
        );
        assert_eq!(
            data_centers(&profile, &settings).unwrap(),
            ["northeurope", "swedencentral"]
        );
        // Creation is not reachable yet: there is no client, and a normal caller is refused.
        let request = super::super::deployment::Request::new(
            "azure-fixture".into(),
            "/synthetic/not-mounted".into(),
            "a".repeat(40),
            profile.clone(),
            "/synthetic/state".into(),
            settings.clone(),
        );
        assert!(compute(&request).is_err());
        assert_eq!(
            preflight("azure-fixture", &profile, &settings).unwrap_err().to_string(),
            "Horizon cannot create clouds on Azure yet"
        );
        const { assert!(!horizon_cloud::offers::AZURE_DEPLOYABLE) };
    }

    #[test]
    fn an_azure_record_holds_no_provider_resources() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::lock(temp.path()).unwrap();
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"azure-fixture","repository":"/synthetic/not-mounted","revision":"a",
            "profile":azure(),"stage":"Validate","operation":{"state":"prepared"},
            "spec":null,"worker":null,"sessions":[]
        }))
        .unwrap();
        assert!(!retained(&store, &state).unwrap());
    }
}
