//! A provider increase and both local journals commit behind one retained intent.
use super::{Deployment, Error, Record, Result, State, Store, Volume, load, save};
use crate::cloud_runtime::settings::Settings;
use horizon_cloud::{
    Cancellation,
    runpod::{RunPod, volumes::growth::Growth},
};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

const JOURNAL: &str = "workspace-growth.json";
const PENDING: &str = "Workspace disk growth is pending; retry the same disk size before other cloud operations";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    deployment: serde_json::Value,
    storage: Record,
    growth: Growth,
    observed: Option<Volume>,
}

pub(in crate::cloud_runtime) fn require_settled(root: &Path) -> Result<()> {
    if root.join(JOURNAL).try_exists()? {
        return Err(Error::Invalid(PENDING));
    }
    Ok(())
}

/// Grow a dedicated CPU cloud's network workspace without replacing its worker.
/// A failed operation retains its target and blocks competing lifecycle operations
/// until this function reconciles that same target. No files are initialized.
/// # Errors
/// Refuses shrinking, unmanaged storage, migrated allocations and profile drift.
pub fn grow_storage(root: &Path, settings: &Settings, size_gb: u16, cancel: &Cancellation) -> Result<Deployment> {
    cancel.check()?;
    let store = Store::lock(root)?;
    let retained = read(&store)?;
    if retained.is_none() {
        settings.credential()?;
    }
    let mut intent = match retained {
        Some(intent) => intent,
        None => prepare(&store, size_gb)?,
    };
    if intent.growth.requested_size() != u32::from(size_gb) {
        return Err(Error::Invalid(PENDING));
    }
    intent.verify(&store)?;
    if intent.recover_confirmation()? {
        write(&store, &intent)?;
    }
    if intent.observed.is_none() {
        let provider = RunPod::new(settings.credential()?);
        let mut growth = intent.growth.clone();
        let volume = provider.grow_volume(&mut growth, cancel, |next| {
            intent.growth = next.clone();
            write(&store, &intent).map_err(|_| horizon_cloud::CloudError::Persistence)
        })?;
        intent.observed = Some(volume);
        write(&store, &intent)?;
    }
    commit(&store, &intent, &mut |_| Ok(()))
}

fn prepare(store: &Store, size: u16) -> Result<Intent> {
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.refuse_pending_replacement()?;
    if state.profile.provider != "runpod" || state.profile.gpu || !state.worker_ready() {
        return Err(Error::Invalid(
            "Disk growth requires a ready CPU cloud with managed network storage",
        ));
    }
    let worker = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    let storage = load(store, worker)?.ok_or(Error::Invalid("Cloud has no managed workspace volume"))?;
    let growth = Growth::new(&storage.spec, &storage.state, u32::from(size))?;
    let intent = Intent {
        version: 1,
        deployment: serde_json::to_value(state).map_err(|_| Error::Json)?,
        storage,
        growth,
        observed: None,
    };
    intent.verify(store)?;
    write(store, &intent)?;
    Ok(intent)
}

impl Intent {
    fn recover_confirmation(&mut self) -> Result<bool> {
        if self.observed.is_some() || !self.growth.confirmed() {
            return Ok(false);
        }
        let State::Bound { volume, .. } = &self.storage.state else {
            return Err(Error::Invalid("Disk growth has no bound storage"));
        };
        let mut volume = volume.clone();
        volume.size = self.growth.requested_size();
        self.observed = Some(volume);
        Ok(true)
    }

    fn original(&self) -> Result<Deployment> {
        serde_json::from_value(self.deployment.clone()).map_err(|_| Error::Json)
    }

    fn next(&self) -> Result<(Deployment, Record)> {
        let volume = self
            .observed
            .as_ref()
            .filter(|_| self.growth.confirmed())
            .ok_or(Error::Invalid(PENDING))?;
        let State::Bound { volume: original, .. } = &self.storage.state else {
            return Err(Error::Invalid("Disk growth has no bound storage"));
        };
        let mut expected = original.clone();
        expected.size = self.growth.requested_size();
        if *volume != expected {
            return Err(Error::Invalid("Grown volume differs from the owned storage"));
        }
        let size = u16::try_from(volume.size).map_err(|_| Error::Invalid("Invalid disk size"))?;
        let mut state = self.original()?;
        state.profile.storage.volume_gb = size;
        let worker = state
            .spec
            .as_mut()
            .ok_or(Error::Invalid("Missing worker specification"))?;
        worker.profile.storage.volume_gb = size;
        if let Some(assigned) = state.worker.as_mut().and_then(|worker| worker.network_volume.as_mut()) {
            if assigned.id.as_deref() != Some(&volume.id) {
                return Err(Error::Invalid("Worker workspace attachment changed"));
            }
            assigned.size = Some(volume.size);
        }
        let mut storage = self.storage.clone();
        storage.worker = worker.clone();
        storage.spec.size = volume.size;
        storage.state = State::Bound {
            volume: volume.clone(),
            creation: None,
        };
        Ok((state, storage))
    }

    fn verify(&self, store: &Store) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Invalid("Unsupported disk growth journal"));
        }
        let original = self.original()?;
        if original.spec.as_ref() != Some(&self.storage.worker)
            || original.profile != self.storage.worker.profile
            || original.profile.provider != "runpod"
            || original.profile.gpu
            || !original.worker_ready()
            || self.storage.worker.operation_id != original.cloud_id
            || self.storage.spec.operation_id != self.storage.worker.operation_id
            || self.storage.spec.size != u32::from(original.profile.storage.volume_gb)
        {
            return Err(Error::Invalid("Disk growth ownership changed"));
        }
        original.refuse_pending_replacement()?;
        self.storage.state.verify(&self.storage.spec)?;
        let State::Bound { volume, .. } = &self.storage.state else {
            return Err(Error::Invalid("Disk growth requires bound storage"));
        };
        let mut worker = original.worker.clone().ok_or(Error::Invalid("Missing worker"))?;
        // REST v2 mount observations omit capacity. The recorded volume supplies
        // only that missing field; its ID and location must still match, and the
        // provider growth operation independently confirms the live capacity.
        if let Some(mount) = &mut worker.network_volume {
            mount.size.get_or_insert(volume.size);
        }
        worker.verify_resources_with_volume(&self.storage.worker, Some(volume))?;
        self.growth.verify_origin(&self.storage.spec, &self.storage.state)?;
        let current = store
            .load_during_storage_growth()?
            .ok_or(Error::Invalid("Missing deployment"))?;
        let current = serde_json::to_value(current).map_err(|_| Error::Json)?;
        let bytes = fs::read(store.root().join("workspace-volume.json"))?;
        let storage: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        let next = self.observed.as_ref().map(|_| self.next()).transpose()?;
        if (current != self.deployment
            && !next
                .as_ref()
                .is_some_and(|(state, _)| serde_json::to_value(state).ok().as_ref() == Some(&current)))
            || (storage != self.storage && !next.as_ref().is_some_and(|(_, state)| *state == storage))
        {
            return Err(Error::Invalid("Disk growth conflicts with changed local records"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Boundary {
    Storage,
    Deployment,
}

fn commit(store: &Store, intent: &Intent, checkpoint: &mut impl FnMut(Boundary) -> Result<()>) -> Result<Deployment> {
    intent.verify(store)?;
    let (state, storage) = intent.next()?;
    save(store, &storage)?;
    checkpoint(Boundary::Storage)?;
    store.save(&state)?;
    checkpoint(Boundary::Deployment)?;
    fs::remove_file(store.root().join(JOURNAL))?;
    #[cfg(unix)]
    fs::File::open(store.root())?.sync_all()?;
    Ok(state)
}

fn read(store: &Store) -> Result<Option<Intent>> {
    match fs::read(store.root().join(JOURNAL)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| Error::Json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write(store: &Store, intent: &Intent) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(store.root())?;
    file.write_all(&serde_json::to_vec(intent).map_err(|_| Error::Json)?)?;
    file.as_file().sync_all()?;
    file.persist(store.root().join(JOURNAL)).map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(store.root())?.sync_all()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
