//! Storage journal shares the deployment lock but retains its own allocation fence.
use super::{Deployment, Error, Event, Result, Store};
use horizon_cloud::{
    Cancellation, CreateState, WorkerSpec,
    runpod::{
        RunPod,
        volumes::{Spec, State, Volume},
    },
};
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Deserialize, Serialize)]
struct Record {
    version: u32,
    worker: WorkerSpec,
    spec: Spec,
    state: State,
}

pub(super) fn prepare(
    provider: &RunPod,
    store: &Store,
    deployment: &Deployment,
    worker: &WorkerSpec,
    cancel: &Cancellation,
) -> Result<Option<Volume>> {
    let mut record = match load(store, worker)? {
        Some(record) => record,
        None if worker.profile.gpu || deployment.operation != CreateState::Prepared => return Ok(None),
        None => {
            let record = Record {
                version: 1,
                worker: worker.clone(),
                spec: provider.workspace_volume_spec(worker, cancel)?,
                state: State::Prepared,
            };
            save(store, &record)?;
            record
        }
    };
    let mut operation = record.state.clone();
    let volume = provider.ensure_volume(&record.spec.clone(), &mut operation, cancel, |next| {
        record.state = next.clone();
        save(store, &record).map_err(|_| horizon_cloud::CloudError::Persistence)
    })?;
    Ok(Some(volume))
}

pub(super) fn expected(store: &Store, worker: &WorkerSpec) -> Result<Option<Volume>> {
    match load(store, worker)? {
        None => Ok(None),
        Some(Record {
            state: State::Bound { volume, .. },
            ..
        }) => Ok(Some(volume)),
        Some(_) => Err(Error::Invalid(
            "Workspace storage is not confirmed; reconcile its allocation or deletion",
        )),
    }
}

pub(in crate::cloud_runtime) fn retained(store: &Store, deployment: &Deployment) -> Result<bool> {
    Ok(load_owned(store.root(), deployment)?.is_some_and(|record| record.state != State::Deleted))
}

/// Drops a journal only after this worker's storage is confirmed deleted.
/// Identity checks match cleanup, so a mismatched journal is left in place.
/// The marker is removed first so a crash cannot look like a missing journal.
pub(in crate::cloud_runtime) fn release_deleted_journal(store: &Store, worker: &WorkerSpec) -> Result<()> {
    let path = store.root().join("workspace-volume.json");
    let marker = store.root().join("workspace-volume.required");
    match load(store, worker)? {
        Some(record) if record.state == State::Deleted => {}
        Some(_) => {
            return Err(Error::Invalid(
                "Managed workspace storage is not confirmed deleted; finish cleanup before redeploying",
            ));
        }
        None => return Ok(()),
    }
    if marker.try_exists()? {
        std::fs::remove_file(&marker)?;
        // Durably drop the marker before the journal. A crash after only the
        // journal unlink would otherwise look like a missing record.
        #[cfg(unix)]
        std::fs::File::open(store.root())?.sync_all()?;
    }
    std::fs::remove_file(&path)?;
    #[cfg(unix)]
    std::fs::File::open(store.root())?.sync_all()?;
    Ok(())
}

pub(super) fn terminate(
    provider: &RunPod,
    store: &Store,
    deployment: &Deployment,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let Some(mut record) = load_owned(store.root(), deployment)? else {
        return Ok(());
    };
    let mut operation = record.state.clone();
    provider.terminate_volume_with_progress(
        &record.spec.clone(),
        &mut operation,
        cancel,
        |next| {
            record.state = next.clone();
            save(store, &record).map_err(|_| horizon_cloud::CloudError::Persistence)
        },
        super::request_detail(emit),
    )?;
    Ok(())
}

pub(in crate::cloud_runtime) fn validate_migration(root: &std::path::Path, deployment: &Deployment) -> Result<()> {
    load_owned(root, deployment).map(|_| ())
}

/// Moves the journal from `from` to `to`, which differ only in the image and its
/// registry credential. An image replacement calls this before saving the deployment
/// record that switches (or restores) its worker specification, so a crash in between
/// leaves a journal that the still-pending replacement accepts. Idempotent.
pub(in crate::cloud_runtime) fn rebind(store: &Store, from: &WorkerSpec, to: &WorkerSpec) -> Result<()> {
    from.verify_replacement(to)?;
    let Some(mut record) = load_at(store.root(), from, Some(to))? else {
        return Ok(());
    };
    if record.worker != *to {
        record.worker = to.clone();
        save(store, &record)?;
    }
    Ok(())
}

fn load(store: &Store, worker: &WorkerSpec) -> Result<Option<Record>> {
    load_at(store.root(), worker, None)
}

/// Loads the journal of the deployment's worker or, while a journaled image replacement
/// has a built image, of the same worker on that image (see `rebind`).
fn load_owned(root: &std::path::Path, deployment: &Deployment) -> Result<Option<Record>> {
    let worker = deployment
        .spec
        .as_ref()
        .ok_or(Error::Invalid("Storage journal has no worker specification"))?;
    load_at(root, worker, deployment.replacement_worker()?.as_ref())
}

/// The journal records `worker` exactly. Only a journaled replacement's worker may
/// differ, in the image digest and registry credential alone and only to its values.
fn records_worker(recorded: &WorkerSpec, worker: &WorkerSpec, replacement: Option<&WorkerSpec>) -> bool {
    recorded == worker || replacement.is_some_and(|next| recorded == next && worker.verify_replacement(next).is_ok())
}

fn load_at(root: &std::path::Path, worker: &WorkerSpec, replacement: Option<&WorkerSpec>) -> Result<Option<Record>> {
    let bytes = match std::fs::read(root.join("workspace-volume.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if root.join("workspace-volume.required").try_exists()? {
                return Err(Error::Invalid(
                    "Workspace storage journal is missing; restore it before cleanup",
                ));
            }
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    if record.version != 1
        || !records_worker(&record.worker, worker, replacement)
        || record.spec.operation_id != worker.operation_id
        || record.spec.size != u32::from(worker.profile.storage.volume_gb)
        || worker.profile.gpu
        || (!worker.data_centers.is_empty() && !worker.data_centers.contains(&record.spec.data_center_id))
    {
        return Err(Error::Invalid("Workspace storage journal does not match this cloud"));
    }
    record.state.verify(&record.spec)?;
    Ok(Some(record))
}
fn save(store: &Store, record: &Record) -> Result<()> {
    let marker = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store.root().join("workspace-volume.required"))?;
    marker.sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(store.root())?.sync_all()?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|_| Error::Json)?;
    let mut file = tempfile::NamedTempFile::new_in(store.root())?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(store.root().join("workspace-volume.json"))
        .map_err(|e| e.error)?;
    #[cfg(unix)]
    std::fs::File::open(store.root())?.sync_all()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::cloud_runtime::state::{OperationId, ReplacementImage};
    use crate::cloud_runtime::{Event, Stage};
    fn record() -> Record {
        let worker: WorkerSpec = serde_json::from_value(serde_json::json!({
            "operation_id":"owned-operation", "image_digest":format!("example/worker@sha256:{}", "a".repeat(64)),
            "profile":{"provider":"runpod","image":"example/worker","cpu":4,"memory_gb":8},
            "public_key":"unused-fixture-key","registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
        })).unwrap();
        Record {
            version: 1,
            spec: Spec {
                operation_id: worker.operation_id.clone(),
                size: 20,
                data_center_id: "EU-TEST-1".into(),
            },
            worker,
            state: State::Prepared,
        }
    }
    fn deployment(worker: &WorkerSpec) -> Deployment {
        serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":worker.operation_id,"repository":"/synthetic","revision":"a",
            "profile":worker.profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
            "spec":worker,"worker":null,"sessions":[]
        }))
        .unwrap()
    }
    /// A deployment whose provider update to image `b` may be in flight.
    fn replacing(worker: &WorkerSpec) -> (Deployment, WorkerSpec) {
        let mut state = deployment(worker);
        state
            .begin_replacement(OperationId::generate(), "c".repeat(40), "horizon-fixture".into())
            .unwrap();
        state
            .replacement_built(ReplacementImage {
                digest: format!("example/worker@sha256:{}", "b".repeat(64)),
                registry_auth_id: Some("pull-b".into()),
                registry_generation: None,
            })
            .unwrap();
        state.request_replacement().unwrap();
        let next = state.replacement_worker().unwrap().unwrap();
        (state, next)
    }
    fn bound(record: &mut Record) -> Volume {
        let volume = Volume {
            id: "volume1".into(),
            name: record.spec.name(),
            size: record.spec.size,
            data_center_id: record.spec.data_center_id.clone(),
        };
        record.state = State::Bound {
            volume: volume.clone(),
            creation: None,
        };
        volume
    }
    #[test]
    fn journal_survives_restart_and_refuses_corruption_or_another_operation() {
        let root = tempfile::tempdir().unwrap();
        let original = record();
        {
            let store = Store::lock(root.path()).unwrap();
            save(&store, &original).unwrap();
        }
        let store = Store::lock(root.path()).unwrap();
        assert!(retained(&store, &deployment(&original.worker)).unwrap());
        assert!(expected(&store, &original.worker).is_err());
        let mut other = original.worker.clone();
        other.operation_id = "another-operation".into();
        assert!(load(&store, &other).is_err());
        std::fs::write(root.path().join("workspace-volume.json"), "invalid").unwrap();
        assert!(load(&store, &original.worker).is_err());
    }
    #[test]
    fn bound_volume_cannot_change_identity_or_location_during_restart() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut record = record();
        let volume = Volume {
            id: "volume1".into(),
            name: record.spec.name(),
            size: record.spec.size,
            data_center_id: record.spec.data_center_id.clone(),
        };
        record.state = State::Bound {
            volume: volume.clone(),
            creation: None,
        };
        save(&store, &record).unwrap();
        assert_eq!(expected(&store, &record.worker).unwrap(), Some(volume.clone()));
        let mut wrong = volume;
        wrong.data_center_id = "different".into();
        record.state = State::Bound {
            volume: wrong,
            creation: None,
        };
        save(&store, &record).unwrap();
        assert!(expected(&store, &record.worker).is_err());
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        assert!(!retained(&store, &deployment(&record.worker)).unwrap());
        assert!(expected(&store, &record.worker).is_err());
    }
    #[test]
    fn reopened_journal_preserves_and_checks_creation_evidence() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut record = record();
        let volume = Volume {
            id: "synthetic-volume".into(),
            name: record.spec.name(),
            size: record.spec.size,
            data_center_id: record.spec.data_center_id.clone(),
        };
        record.state = serde_json::from_value(serde_json::json!({
            "state":"bound", "volume":volume,
            "creation":{"version":1,"spec":record.spec,"volume":volume}
        }))
        .unwrap();
        save(&store, &record).unwrap();
        let reopened = load(&store, &record.worker).unwrap().unwrap();
        assert_eq!(reopened.state, record.state);
        assert!(reopened.state.creation_receipt(&record.spec).unwrap().is_some());
        let mut changed = serde_json::to_value(&record).unwrap();
        changed["state"]["creation"]["volume"]["id"] = serde_json::json!("another-volume");
        std::fs::write(
            store.root().join("workspace-volume.json"),
            serde_json::to_vec(&changed).unwrap(),
        )
        .unwrap();
        assert!(expected(&store, &record.worker).is_err());
        assert!(validate_migration(store.root(), &deployment(&record.worker)).is_err());
    }
    #[test]
    fn removal_requires_storage_cleanup_even_without_a_live_worker() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut record = record();
        let volume = Volume {
            id: "volume1".into(),
            name: record.spec.name(),
            size: record.spec.size,
            data_center_id: record.spec.data_center_id.clone(),
        };
        let mut deployment: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":record.worker.operation_id,"repository":"/synthetic","revision":"a",
            "profile":record.worker.profile,"stage":"Provision","operation":{"state":"prepared"},
            "spec":record.worker,"worker":null,"sessions":[]
        }))
        .unwrap();
        for (operation, storage) in [
            (
                CreateState::Prepared,
                State::Bound {
                    volume: volume.clone(),
                    creation: None,
                },
            ),
            (
                CreateState::Terminated {
                    worker_id: "worker1".into(),
                },
                State::Deleting { volume, creation: None },
            ),
        ] {
            deployment.operation = operation;
            record.state = storage;
            save(&store, &record).unwrap();
            assert!(!crate::cloud_runtime::lifecycle::can_remove(&store, &deployment).unwrap());
        }
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &deployment).unwrap());
        std::fs::remove_file(store.root().join("workspace-volume.json")).unwrap();
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &deployment).is_err());
        std::fs::write(store.root().join("workspace-volume.json"), "corrupt").unwrap();
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &deployment).is_err());
    }
    #[test]
    fn deleted_journal_can_be_released_and_other_states_cannot() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut record = record();
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        let mut other = record.worker.clone();
        other.operation_id = "another-operation".into();
        assert!(release_deleted_journal(&store, &other).is_err());
        assert!(root.path().join("workspace-volume.json").exists());
        record.state = State::Deleting {
            volume: Volume {
                id: "volume1".into(),
                name: record.spec.name(),
                size: record.spec.size,
                data_center_id: record.spec.data_center_id.clone(),
            },
            creation: None,
        };
        save(&store, &record).unwrap();
        assert!(release_deleted_journal(&store, &record.worker).is_err());
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        release_deleted_journal(&store, &record.worker).unwrap();
        assert!(!root.path().join("workspace-volume.json").exists());
        assert!(!root.path().join("workspace-volume.required").exists());
        release_deleted_journal(&store, &record.worker).unwrap();
        std::fs::write(root.path().join("workspace-volume.required"), b"").unwrap();
        assert!(release_deleted_journal(&store, &record.worker).is_err());
    }
    #[test]
    fn absent_journal_keeps_legacy_worker_storage_validation() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        assert_eq!(expected(&store, &record().worker).unwrap(), None);
        assert!(!retained(&store, &deployment(&record().worker)).unwrap());
    }
    #[test]
    fn prepared_cleanup_recovers_when_storage_finished_before_deployment_save() {
        for journal_deleted in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut record = record();
            record.state = State::Deleted;
            let deployment: Deployment = serde_json::from_value(serde_json::json!({
                "version":1,"cloud_id":record.worker.operation_id,"repository":"/synthetic","revision":"a",
                "profile":record.worker.profile,"stage":"Provision","operation":{"state":"prepared"},
                "spec":record.worker,"worker":null,"sessions":[]
            }))
            .unwrap();
            {
                let store = Store::lock(root.path()).unwrap();
                store.save(&deployment).unwrap();
                if journal_deleted {
                    save(&store, &record).unwrap();
                }
            }
            let mut key = tempfile::NamedTempFile::new().unwrap();
            key.write_all(b"synthetic-test-key").unwrap();
            let settings = serde_json::from_value(serde_json::json!({
                "runpod_key_file":key.path(),"ssh_identity_file":root.path().join("ssh"),
                "docker_config":root.path().join("docker"),"registry_pull_auth_id":null,
                "cpu_flavors":[],"gpu_types":[]
            }))
            .unwrap();
            // The second pass is cancelled from the start: cancellation ends once
            // deletion starts, so it cannot fail an accepted storage delete.
            for cancelled in [false, true] {
                let cancel = Cancellation::default();
                if cancelled {
                    cancel.cancel();
                }
                let events = std::cell::RefCell::new(Vec::new());
                super::super::terminate(root.path(), &settings, &cancel, &|event| {
                    events.borrow_mut().push(event);
                })
                .unwrap();
                let reported: Vec<_> = events
                    .into_inner()
                    .into_iter()
                    .filter_map(|event| match event {
                        Event::Stage(stage, _) => Some(stage.label().to_owned()),
                        Event::Progress(progress) => Some(progress.detail),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    reported,
                    [
                        "Delete workspace storage",
                        "Deleting managed workspace storage and confirming its removal"
                    ]
                );
                let store = Store::lock(root.path()).unwrap();
                let saved = store.load().unwrap().unwrap();
                assert_eq!(saved.stage, Stage::Deleted);
                assert_eq!(saved.operation, CreateState::Prepared);
                assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &saved).unwrap());
            }
        }
    }
    #[test]
    fn deletion_steps_are_never_saved() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let record = record();
        let mut deployment: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":record.worker.operation_id,"repository":"/synthetic","revision":"a",
            "profile":record.worker.profile,"stage":"Ready","operation":{"state":"bound","worker_id":"worker1"},
            "spec":record.worker,"worker":null,"sessions":[]
        }))
        .unwrap();
        store.save(&deployment).unwrap();
        for stage in Stage::DELETION {
            deployment.stage = stage;
            assert!(matches!(store.save(&deployment), Err(Error::Json)));
            assert!(serde_json::from_value::<Stage>(serde_json::json!(format!("{stage:?}"))).is_err());
        }
        assert_eq!(store.load().unwrap().unwrap().stage, Stage::Ready);
    }
    #[cfg(unix)]
    #[test]
    fn removal_without_worker_spec_propagates_journal_metadata_errors() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let record = record();
        let deployment: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":record.worker.operation_id,"repository":"/synthetic","revision":"a",
            "profile":record.worker.profile,"stage":"Deleted","operation":{"state":"prepared"},
            "spec":null,"worker":null,"sessions":[]
        }))
        .unwrap();
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &deployment).unwrap());
        for name in ["workspace-volume.json", "workspace-volume.required"] {
            let path = store.root().join(name);
            std::os::unix::fs::symlink(name, &path).unwrap();
            assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &deployment).is_err());
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn replacement_commit_rebinds_the_journal_before_the_deployment() {
        for interrupted in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let store = Store::lock(root.path()).unwrap();
            let mut record = record();
            let volume = bound(&mut record);
            save(&store, &record).unwrap();
            let (mut state, next) = replacing(&record.worker);
            store.save(&state).unwrap();
            assert!(expected(&store, &next).is_err());
            assert!(retained(&store, &state).unwrap());
            if interrupted {
                // The commit's journal write landed but its deployment write did not.
                rebind(&store, &record.worker, &next).unwrap();
                let rebound = std::fs::read(root.path().join("workspace-volume.json")).unwrap();
                assert!(load(&store, &record.worker).is_err());
                assert!(retained(&store, &state).unwrap());
                validate_migration(root.path(), &state).unwrap();
                let mut unjournaled = state.clone();
                unjournaled.image_replacement = None;
                unjournaled.stage = crate::cloud_runtime::Stage::Ready;
                assert!(retained(&store, &unjournaled).is_err());
                rebind(&store, &record.worker, &next).unwrap();
                assert_eq!(
                    std::fs::read(root.path().join("workspace-volume.json")).unwrap(),
                    rebound
                );
            }
            let operation = state.image_replacement.as_ref().unwrap().operation;
            assert_eq!(super::super::commit_replacement(&store, &mut state).unwrap(), operation);
            let saved = store.load().unwrap().unwrap();
            assert_eq!(saved.spec.as_ref(), Some(&next));
            assert!(saved.image_replacement.is_none());
            assert_eq!(saved.session_restart, Some(operation));
            assert_eq!(saved.stage, crate::cloud_runtime::Stage::Readiness);
            assert_eq!(expected(&store, &next).unwrap(), Some(volume));
            assert!(super::super::commit_replacement(&store, &mut state).is_err());
            assert_eq!(store.load().unwrap().unwrap().session_restart, Some(operation));
        }
    }
    #[test]
    fn journal_accepts_only_the_worker_on_its_journaled_replacement_image() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut record = record();
        let (state, next) = replacing(&record.worker);
        rebind(&store, &record.worker, &next).unwrap();
        assert!(!root.path().join("workspace-volume.json").exists());
        let mut third = next.clone();
        third.image_digest = format!("example/worker@sha256:{}", "c".repeat(64));
        record.worker = third.clone();
        save(&store, &record).unwrap();
        assert!(retained(&store, &state).is_err());
        // A journal that cannot be rebound keeps the replacement requested.
        store.save(&state).unwrap();
        let mut committing = state.clone();
        assert!(super::super::commit_replacement(&store, &mut committing).is_err());
        assert_eq!(committing.spec, state.spec);
        assert_eq!(
            store.load().unwrap().unwrap().image_replacement,
            state.image_replacement
        );
        let mut rekeyed = next.clone();
        rekeyed.public_key = "another-key".into();
        record.worker = next.clone();
        save(&store, &record).unwrap();
        let journal = std::fs::read(root.path().join("workspace-volume.json")).unwrap();
        for (from, to) in [
            (&next, &rekeyed),
            (&next, &next),
            (&third, &state.spec.clone().unwrap()),
        ] {
            assert!(rebind(&store, from, to).is_err());
            assert_eq!(
                std::fs::read(root.path().join("workspace-volume.json")).unwrap(),
                journal
            );
        }
        let mut prepared = deployment(state.spec.as_ref().unwrap());
        prepared
            .begin_replacement(OperationId::generate(), "c".repeat(40), "horizon-fixture".into())
            .unwrap();
        assert!(
            retained(&store, &prepared).is_err(),
            "no image was built for the journal to switch to"
        );
        // Cancelling a switched replacement restores the previous binding.
        let current = state.spec.clone().unwrap();
        rebind(&store, &next, &current).unwrap();
        assert!(load(&store, &current).unwrap().is_some());
        assert!(retained(&store, &deployment(&current)).unwrap());
    }
    #[test]
    fn deletion_accepts_a_journal_rebound_by_an_interrupted_commit() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut record = record();
        let (mut state, next) = replacing(&record.worker);
        state.operation = CreateState::Terminated {
            worker_id: "worker1".into(),
        };
        record.worker = next;
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        let provider = RunPod::new(horizon_cloud::Credential::new("synthetic-test-key".into()).unwrap());
        terminate(&provider, &store, &state, &Cancellation::default(), &|_| {}).unwrap();
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &state).unwrap());
        // Finishing the deletion drops the journal; removal and a later redeploy then
        // compare the storage journal with the recorded worker only.
        super::super::drop_replacement(&store, &mut state).unwrap();
        state.stage = crate::cloud_runtime::Stage::Deleted;
        assert!(state.image_replacement.is_none());
        assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &state).unwrap());
        release_deleted_journal(&store, state.spec.as_ref().unwrap()).unwrap();
    }
}
