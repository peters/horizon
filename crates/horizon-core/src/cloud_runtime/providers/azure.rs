//! Azure's place among the providers. The machine's `azure` settings name the VM
//! sizes and regions from the catalog, but Horizon cannot create, manage or delete an
//! Azure cloud yet (#1774), so every lifecycle operation refuses before it writes a
//! record, tells an observer or reaches a provider.
use super::{super::Error, Cancellation, CreateState, Deployment, Event, Lifecycle, Observer, Result, Settings, Store};
use horizon_cloud::runpod::recovery::Reconciliation;

/// What every Azure lifecycle operation answers.
pub(in crate::cloud_runtime) const REFUSED: Error = Error::Invalid("Horizon cannot manage Azure clouds yet");
/// What a new Azure cloud answers before any record is written.
pub(super) const NOT_CREATABLE: Error = Error::Invalid("Horizon cannot create clouds on Azure yet");
/// What asks for an Azure client. Distinct from a missing account, which a key could fix.
pub(super) const NO_CLIENT: Error = Error::Invalid("Horizon has no Azure deployment client yet");

/// The VM sizes Horizon tries, in order; the spec keeps them as its CPU flavors.
pub(super) fn cpu_flavors(settings: &Settings) -> Result<Vec<String>> {
    Ok(settings.azure_settings()?.vm_sizes.clone())
}

/// The regions an Azure cloud may be made in, in order.
pub(super) fn data_centers(settings: &Settings) -> Result<Vec<String>> {
    Ok(settings.azure_settings()?.regions.clone())
}

pub(super) struct Azure;

impl Lifecycle for Azure {
    fn check(
        &self,
        _store: &Store,
        _state: &mut Deployment,
        _worker_hint: Option<&str>,
        _cancel: &Cancellation,
    ) -> Result<(Reconciliation, CreateState)> {
        Err(REFUSED)
    }

    fn stop(
        &self,
        _store: &Store,
        _state: &mut Deployment,
        _cancel: &Cancellation,
        _observe: Observer<'_>,
    ) -> Result<()> {
        Err(REFUSED)
    }

    fn resume(
        &self,
        _store: &Store,
        _state: &mut Deployment,
        _cancel: &Cancellation,
        _observe: Observer<'_>,
    ) -> Result<()> {
        Err(REFUSED)
    }

    fn release_devices(&self, _store: &Store, _state: &mut Deployment, _cancel: &Cancellation) -> Result<()> {
        Err(REFUSED)
    }

    fn delete(
        &self,
        _store: &Store,
        _state: &mut Deployment,
        _cancel: &Cancellation,
        _emit: &dyn Fn(Event),
    ) -> Result<()> {
        Err(REFUSED)
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

    fn files(root: &std::path::Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn every_lifecycle_operation_refuses_and_leaves_the_record_untouched() {
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
        let refusals = [
            Azure.check(&store, &mut state, None, &cancel).map(drop),
            Azure.stop(&store, &mut state, &cancel, &observe),
            Azure.resume(&store, &mut state, &cancel, &observe),
            Azure.release_devices(&store, &mut state, &cancel),
            Azure.delete(&store, &mut state, &cancel, &emit),
        ];
        for (index, refusal) in refusals.into_iter().enumerate() {
            let error = refusal.unwrap_err();
            assert_eq!(error.to_string(), "Horizon cannot manage Azure clouds yet", "{index}");
        }
        assert_eq!((observed.get(), emitted.get()), (0, 0));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        assert_eq!(std::fs::read(temp.path().join("deployment.json")).unwrap(), saved);
        assert_eq!(files(temp.path()), listed, "no journal or other file is written");
    }
}
