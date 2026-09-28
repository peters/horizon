//! CPU replacement commits its new worker and storage binding under one journal.
use super::{Deployment, Error, Record, Result, State, Store, load, save};
use crate::cloud_runtime::{
    Event, Stage,
    settings::{Settings, validate_ssh_identity},
};
use horizon_cloud::{
    Cancellation, CreateState, Worker,
    runpod::{RunPod, resize::Replacement},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const JOURNAL: &str = "compute-resize.json";
const PENDING: &str = "Compute resize is pending; retry the same CPU and memory before other cloud operations";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    deployment: serde_json::Value,
    storage: Record,
    replacement: Replacement,
    observed: Option<Worker>,
}

pub(in crate::cloud_runtime) fn require_settled(root: &Path) -> Result<()> {
    if root.join(JOURNAL).try_exists()? {
        return Err(Error::Invalid(PENDING));
    }
    Ok(())
}

pub(super) fn pending(store: &Store) -> Result<Option<super::ResizeTarget>> {
    read(store)?
        .map(|intent| {
            intent.verify(store)?;
            let profile = &intent.replacement.specification().profile;
            Ok(super::ResizeTarget::Compute {
                cpu: profile.cpu,
                memory_gb: profile.memory_gb,
            })
        })
        .transpose()
}

/// Replace a ready CPU worker while retaining its network workspace and recorded sessions.
/// The old worker is deleted only after an identity-checked replacement intent is durable.
/// Retry with the same size to recover an interrupted operation.
/// # Errors
/// Refuses unmanaged/GPU storage, changed ownership or profiles, active remote-device grants,
/// and competing lifecycle operations. Provider uncertainty retains the journal.
pub fn resize_compute(
    root: &Path,
    settings: &Settings,
    cpu: u16,
    memory_gb: u16,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<Deployment> {
    resize_compute_with(root, settings, cpu, memory_gb, cancel, emit, || {
        super::super::reconnect(root, settings.clone(), cancel, emit)
    })
}

fn resize_compute_with(
    root: &Path,
    settings: &Settings,
    cpu: u16,
    memory_gb: u16,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
    reconnect: impl FnOnce() -> Result<Deployment>,
) -> Result<Deployment> {
    cancel.check()?;
    {
        let store = Store::lock(root)?;
        super::growth::require_settled(root)?;
        let retained = read(&store)?;
        if retained.is_none() {
            let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
            if (state.profile.cpu, state.profile.memory_gb) == (cpu, memory_gb) {
                state.refuse_pending_replacement()?;
                if state.profile.provider == "runpod"
                    && !state.profile.gpu
                    && state.source_ready
                    && !state.stop_requested
                    && matches!(state.operation, CreateState::Bound { .. })
                {
                    drop(store);
                    return reconnect();
                }
                return Err(Error::Invalid("Choose a different CPU or memory size"));
            }
            let spec = state
                .spec
                .as_ref()
                .ok_or(Error::Invalid("Missing worker specification"))?;
            verify_identity(settings, &spec.public_key, cancel)?;
            settings.credential()?;
        }
        let mut intent = match retained {
            Some(intent) => intent,
            None => prepare(&store, settings, cpu, memory_gb)?,
        };
        let profile = &intent.replacement.specification().profile;
        if (profile.cpu, profile.memory_gb) != (cpu, memory_gb) {
            return Err(Error::Invalid(PENDING));
        }
        intent.verify(&store)?;
        verify_identity(settings, &intent.replacement.specification().public_key, cancel)?;
        if intent.observed.is_none() {
            let provider = RunPod::new(settings.credential()?);
            let mut replacement = intent.replacement.clone();
            emit(Event::stage(Stage::Provision));
            let result = provider.resize_cpu(
                &mut replacement,
                cancel,
                |next| {
                    intent.replacement = next.clone();
                    write(&store, &intent).map_err(|_| horizon_cloud::CloudError::Persistence)
                },
                |progress| emit(Event::Output(format!("{progress:?}"))),
            );
            record_observation(&store, &mut intent, result)?;
        }
        commit(&store, &intent, &mut |_| Ok(()))?;
    }
    reconnect()
}

fn verify_identity(settings: &Settings, expected: &str, cancel: &Cancellation) -> Result<()> {
    const LIMIT: usize = 64 * 1024;
    validate_ssh_identity(&settings.ssh_identity_file)?;
    let invalid = || Error::Invalid("The SSH private identity must match the replacement worker");
    if super::super::current_public_key(&settings.ssh_identity_file)? != expected {
        return Err(invalid());
    }
    let mut bytes = zeroize::Zeroizing::new(Vec::with_capacity(LIMIT + 1));
    fs::File::open(&settings.ssh_identity_file)?
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(invalid());
    }
    let runner = crate::cloud_runtime::command::Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let derived = crate::cloud_runtime::bootstrap_recovery::connection::public_identity(&bytes, &runner)
        .map_err(|_| invalid())?;
    if !derived
        .split_whitespace()
        .take(2)
        .eq(expected.split_whitespace().take(2))
    {
        return Err(invalid());
    }
    Ok(())
}

fn record_observation(
    store: &Store,
    intent: &mut Intent,
    result: std::result::Result<Worker, horizon_cloud::CloudError>,
) -> Result<()> {
    match result {
        Ok(worker) => {
            intent.observed = Some(worker);
            write(store, intent)
        }
        Err(error) => {
            if intent.replacement.unstarted() {
                fs::remove_file(store.root().join(JOURNAL))?;
                #[cfg(unix)]
                fs::File::open(store.root())?.sync_all()?;
            }
            Err(error.into())
        }
    }
}

fn prepare(store: &Store, settings: &Settings, cpu: u16, memory_gb: u16) -> Result<Intent> {
    let state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    state.refuse_pending_replacement()?;
    if state.profile.provider != "runpod" || state.profile.gpu || !state.worker_ready() || state.stop_requested {
        return Err(Error::Invalid(
            "Compute resize requires a ready CPU cloud with managed network storage",
        ));
    }
    if state.requires_browserstack_release() {
        return Err(Error::Invalid(
            "Release this cloud's remote devices before resizing compute",
        ));
    }
    let current = state
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Missing worker specification"))?;
    let worker = state.worker.as_ref().ok_or(Error::Invalid("Missing worker"))?;
    if state.operation
        != (CreateState::Bound {
            worker_id: worker.id.clone(),
        })
    {
        return Err(Error::Invalid("Worker allocation differs"));
    }
    let storage = load(store, current)?.ok_or(Error::Invalid("Cloud has no managed workspace volume"))?;
    let State::Bound { volume, .. } = &storage.state else {
        return Err(Error::Invalid("Storage is not bound"));
    };
    let mut next = current.clone();
    next.profile.cpu = cpu;
    next.profile.memory_gb = memory_gb;
    next.cpu_flavors = super::super::sizing::cpu_flavors(&next.profile, settings)?;
    let replacement = Replacement::new(current.clone(), next, worker.id.clone(), volume.clone())?;
    let intent = Intent {
        version: 1,
        deployment: serde_json::to_value(state).map_err(|_| Error::Json)?,
        storage,
        replacement,
        observed: None,
    };
    intent.verify(store)?;
    write(store, &intent)?;
    Ok(intent)
}

impl Intent {
    fn original(&self) -> Result<Deployment> {
        serde_json::from_value(self.deployment.clone()).map_err(|_| Error::Json)
    }
    fn next(&self) -> Result<(Deployment, Record)> {
        let worker = self
            .observed
            .as_ref()
            .filter(|_| self.replacement.completed())
            .ok_or(Error::Invalid(PENDING))?;
        self.replacement.verify_result(worker)?;
        let spec = self.replacement.specification().clone();
        let mut state = self.original()?;
        state.profile = spec.profile.clone();
        state.spec = Some(spec.clone());
        state.operation = CreateState::Bound {
            worker_id: worker.id.clone(),
        };
        state.worker = Some(worker.clone());
        state.stage = Stage::Readiness;
        state.last_self_stop = None;
        let mut storage = self.storage.clone();
        storage.worker = spec;
        storage.state = State::Bound {
            volume: self.replacement.volume().clone(),
            creation: None,
        };
        Ok((state, storage))
    }
    fn verify(&self, store: &Store) -> Result<()> {
        if self.version != 1 {
            return Err(Error::Invalid("Unsupported compute resize journal"));
        }
        let original = self.original()?;
        let current = original
            .spec
            .as_ref()
            .ok_or(Error::Invalid("Missing worker specification"))?;
        let worker = original.worker.as_ref().ok_or(Error::Invalid("Missing worker"))?;
        if original.profile != current.profile
            || original.cloud_id != current.operation_id
            || !original.worker_ready()
            || original.stop_requested
            || original.requires_browserstack_release()
            || original.operation
                != (CreateState::Bound {
                    worker_id: worker.id.clone(),
                })
            || self.storage.worker != *current
        {
            return Err(Error::Invalid("Compute resize original records differ"));
        }
        original.refuse_pending_replacement()?;
        let State::Bound { volume, .. } = &self.storage.state else {
            return Err(Error::Invalid("Storage is not bound"));
        };
        self.storage.state.verify(&self.storage.spec)?;
        self.replacement.verify_origin(current, &worker.id, volume)?;
        let mut worker = worker.clone();
        if let Some(mount) = &mut worker.network_volume {
            mount.size.get_or_insert(volume.size);
        }
        worker.verify(current)?;
        worker.verify_resources_with_volume(current, Some(volume))?;
        let state = store
            .load_during_storage_growth()?
            .ok_or(Error::Invalid("Missing deployment"))?;
        let state = serde_json::to_value(state).map_err(|_| Error::Json)?;
        let storage: Record =
            serde_json::from_slice(&fs::read(store.root().join("workspace-volume.json"))?).map_err(|_| Error::Json)?;
        let next = self.observed.as_ref().map(|_| self.next()).transpose()?;
        if (state != self.deployment
            && !next
                .as_ref()
                .is_some_and(|(s, _)| serde_json::to_value(s).ok().as_ref() == Some(&state)))
            || (storage != self.storage && !next.as_ref().is_some_and(|(_, s)| *s == storage))
        {
            return Err(Error::Invalid("Compute resize conflicts with changed local records"));
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
    file.persist(store.root().join(JOURNAL)).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(store.root())?.sync_all()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
