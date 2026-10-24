//! Azure's lifecycle: `deployment::azure` over the ARM client, with the cloud's
//! records beside the deployment. A worker is provisioned behind the `Compute` seam
//! from the catalog's sizes and regions in the machine's `azure` settings; a stop
//! deallocates its VM, Resume starts the same VM, and a delete proves its resource
//! group absent.
use super::{
    super::{Error, deployment},
    Cancellation, CreateState, Deployment, Event, Lifecycle, Observer, Result, Settings, Store,
};
use horizon_cloud::{Profile, azure::catalog::VM_SIZES, runpod::recovery::Reconciliation};

/// What a companion's operation on an Azure cloud answers until companions admit it.
pub(in crate::cloud_runtime) const REFUSED: Error = Error::Invalid("Horizon cannot manage Azure clouds yet");

/// Checks what an Azure cloud needs before any record, build or request: a workspace
/// disk Azure can make, an id that can name the cloud's resource group, a readable
/// token, and a VM size in the settings that fits the profile.
pub(super) fn preflight(cloud_id: &str, profile: &Profile, settings: &Settings) -> Result<()> {
    let (smallest, largest) = horizon_cloud::provider::AZURE.cpu_volume_gb;
    if !(smallest..=largest).contains(&u32::from(profile.storage.volume_gb)) {
        return Err(Error::Invalid(
            "An Azure workspace disk must be between 4 and 32,767 GB",
        ));
    }
    let group = horizon_cloud::azure::resource_group_name(cloud_id);
    if !horizon_cloud::valid_id(cloud_id) || !horizon_cloud::azure::url::valid_resource_group_name(&group) {
        return Err(Error::Invalid(
            "An Azure cloud needs an ID of letters, digits, hyphens and underscores, at most 79 characters",
        ));
    }
    let azure = settings.azure_settings()?;
    azure.credential()?;
    let fits = |name: &String| {
        VM_SIZES
            .iter()
            .any(|size| size.name == name && size.vcpu >= profile.cpu && size.memory_gb >= profile.memory_gb)
    };
    if !azure.vm_sizes.iter().any(fits) {
        return Err(Error::Invalid(
            "No Azure VM size in the settings has the profile's CPU and memory",
        ));
    }
    Ok(())
}

/// The VM sizes Horizon tries, in order; the spec keeps them as its CPU flavors.
pub(super) fn cpu_flavors(settings: &Settings) -> Result<Vec<String>> {
    Ok(settings.azure_settings()?.vm_sizes.clone())
}

/// The regions an Azure cloud may be made in, in order.
pub(super) fn data_centers(settings: &Settings) -> Result<Vec<String>> {
    Ok(settings.azure_settings()?.regions.clone())
}

/// The resource group, which is recorded before it is requested and kept until its
/// absence is proven.
pub(super) fn retained(store: &Store) -> Result<bool> {
    deployment::azure::retained(store.root())
}

pub(super) struct Azure<'a> {
    pub(super) settings: &'a Settings,
}

impl Lifecycle for Azure<'_> {
    fn check(
        &self,
        store: &Store,
        state: &mut Deployment,
        worker_hint: Option<&str>,
        cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)> {
        let client = deployment::azure::client(self.settings, store)?;
        let report = deployment::azure::check(&client, store, state, worker_hint, cancel)?;
        Ok((report, state.operation.clone()))
    }

    fn stop(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, observe: Observer<'_>) -> Result<()> {
        let client = deployment::azure::client(self.settings, store)?;
        deployment::azure::stop(&client, store, state, cancel, observe)
    }

    fn resume(
        &self,
        store: &Store,
        state: &mut Deployment,
        cancel: &Cancellation,
        observe: Observer<'_>,
    ) -> Result<()> {
        let client = deployment::azure::client(self.settings, store)?;
        deployment::azure::resume(&client, store, state, cancel, observe)
    }

    /// Azure profiles cannot request hosted devices, so an Azure cloud has none to
    /// release; nothing here may reach another provider for it.
    fn release_devices(&self, _store: &Store, _state: &mut Deployment, _cancel: &Cancellation) -> Result<()> {
        Err(Error::Invalid("Azure clouds hold no hosted devices to release"))
    }

    fn delete(&self, store: &Store, state: &mut Deployment, cancel: &Cancellation, emit: &dyn Fn(Event)) -> Result<()> {
        deployment::deletion::delete_azure(store, state, self.settings, cancel, emit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn record() -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"azure-fixture","repository":"/synthetic/not-mounted","revision":"a".repeat(40),
            "profile":{"provider":"azure","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
            "spec":null,"worker":null,"sessions":[]
        }))
        .unwrap()
    }

    fn settings() -> Settings {
        serde_json::from_value(serde_json::json!({
            "runpod_key_file":"unused","ssh_identity_file":"unused","docker_config":"unused",
            "registry_pull_auth_id":null,"cpu_flavors":["cpu3c"],"gpu_types":[],"data_centers":["EU-RO-1"]
        }))
        .unwrap()
    }

    fn files(root: &std::path::Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn without_azure_settings_every_operation_refuses_and_leaves_the_record_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::lock(temp.path()).unwrap();
        let mut state = record();
        store.save(&state).unwrap();
        let saved = std::fs::read(temp.path().join("deployment.json")).unwrap();
        let listed = files(temp.path());
        let before = serde_json::to_value(&state).unwrap();
        let observed = Cell::new(0);
        let observe = |_| {
            observed.set(observed.get() + 1);
            Ok(super::super::super::mutation::State::Settled)
        };
        let emitted = Cell::new(0);
        let emit = |_: Event| emitted.set(emitted.get() + 1);
        let cancel = Cancellation::default();
        let settings = settings();
        let azure = Azure { settings: &settings };
        let missing = "Add an azure section to the cloud settings before deploying an Azure cloud";
        let refusals = [
            (azure.check(&store, &mut state, None, &cancel).map(drop), missing),
            (azure.stop(&store, &mut state, &cancel, &observe), missing),
            (azure.resume(&store, &mut state, &cancel, &observe), missing),
            (
                azure.release_devices(&store, &mut state, &cancel),
                "Azure clouds hold no hosted devices to release",
            ),
            (azure.delete(&store, &mut state, &cancel, &emit), missing),
        ];
        for (index, (refusal, expected)) in refusals.into_iter().enumerate() {
            assert_eq!(refusal.unwrap_err().to_string(), expected, "{index}");
        }
        assert_eq!((observed.get(), emitted.get()), (0, 0));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(std::fs::read(temp.path().join("deployment.json")).unwrap(), saved);
        assert_eq!(files(temp.path()), listed, "no journal or other file is written");
    }
}
