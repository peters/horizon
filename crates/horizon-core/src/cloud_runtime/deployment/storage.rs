//! Storage journal shares the deployment lock but retains its own allocation fence.
use super::{Deployment, Error, Result, Store};
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
            state: State::Bound { volume },
            ..
        }) => Ok(Some(volume)),
        Some(_) => Err(Error::Invalid(
            "Workspace storage is not confirmed; reconcile its allocation or deletion",
        )),
    }
}

pub(in crate::cloud_runtime) fn retained(store: &Store, worker: &WorkerSpec) -> Result<bool> {
    Ok(load(store, worker)?.is_some_and(|record| record.state != State::Deleted))
}

/// Drops a journal only after this operation's storage is recorded as deleted.
/// The marker is removed first so a crash cannot look like a missing journal.
pub(in crate::cloud_runtime) fn release_deleted_journal(store: &Store, operation_id: &str) -> Result<()> {
    let path = store.root().join("workspace-volume.json");
    let marker = store.root().join("workspace-volume.required");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if marker.try_exists()? {
                return Err(Error::Invalid(
                    "Workspace storage journal is missing; restore it before redeploying",
                ));
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    if record.version != 1
        || record.state != State::Deleted
        || record.worker.operation_id != operation_id
        || record.spec.operation_id != operation_id
    {
        return Err(Error::Invalid(
            "Managed workspace storage is not confirmed deleted; finish cleanup before redeploying",
        ));
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

pub(super) fn terminate(provider: &RunPod, store: &Store, worker: &WorkerSpec, cancel: &Cancellation) -> Result<()> {
    let Some(mut record) = load(store, worker)? else {
        return Ok(());
    };
    let mut operation = record.state.clone();
    provider.terminate_volume(&record.spec.clone(), &mut operation, cancel, |next| {
        record.state = next.clone();
        save(store, &record).map_err(|_| horizon_cloud::CloudError::Persistence)
    })?;
    Ok(())
}

pub(in crate::cloud_runtime) fn validate_migration(root: &std::path::Path, worker: &WorkerSpec) -> Result<()> {
    load_at(root, worker).map(|_| ())
}

fn load(store: &Store, worker: &WorkerSpec) -> Result<Option<Record>> {
    load_at(store.root(), worker)
}

fn load_at(root: &std::path::Path, worker: &WorkerSpec) -> Result<Option<Record>> {
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
        || record.worker != *worker
        || record.spec.operation_id != worker.operation_id
        || record.spec.size != u32::from(worker.profile.storage.volume_gb)
        || worker.profile.gpu
        || (!worker.data_centers.is_empty() && !worker.data_centers.contains(&record.spec.data_center_id))
    {
        return Err(Error::Invalid("Workspace storage journal does not match this cloud"));
    }
    if let State::Bound { volume } | State::Deleting { volume } = &record.state {
        volume.verify(&record.spec)?;
    }
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
    #[test]
    fn journal_survives_restart_and_refuses_corruption_or_another_operation() {
        let root = tempfile::tempdir().unwrap();
        let original = record();
        {
            let store = Store::lock(root.path()).unwrap();
            save(&store, &original).unwrap();
        }
        let store = Store::lock(root.path()).unwrap();
        assert!(retained(&store, &original.worker).unwrap());
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
        record.state = State::Bound { volume: volume.clone() };
        save(&store, &record).unwrap();
        assert_eq!(expected(&store, &record.worker).unwrap(), Some(volume.clone()));
        let mut wrong = volume;
        wrong.data_center_id = "different".into();
        record.state = State::Bound { volume: wrong };
        save(&store, &record).unwrap();
        assert!(expected(&store, &record.worker).is_err());
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        assert!(!retained(&store, &record.worker).unwrap());
        assert!(expected(&store, &record.worker).is_err());
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
            (CreateState::Prepared, State::Bound { volume: volume.clone() }),
            (
                CreateState::Terminated {
                    worker_id: "worker1".into(),
                },
                State::Deleting { volume },
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
        assert!(release_deleted_journal(&store, "another-operation").is_err());
        assert!(root.path().join("workspace-volume.json").exists());
        record.state = State::Deleting {
            volume: Volume {
                id: "volume1".into(),
                name: record.spec.name(),
                size: record.spec.size,
                data_center_id: record.spec.data_center_id.clone(),
            },
        };
        save(&store, &record).unwrap();
        assert!(release_deleted_journal(&store, &record.worker.operation_id).is_err());
        record.state = State::Deleted;
        save(&store, &record).unwrap();
        release_deleted_journal(&store, &record.worker.operation_id).unwrap();
        assert!(!root.path().join("workspace-volume.json").exists());
        assert!(!root.path().join("workspace-volume.required").exists());
        release_deleted_journal(&store, &record.worker.operation_id).unwrap();
        std::fs::write(root.path().join("workspace-volume.required"), b"").unwrap();
        assert!(release_deleted_journal(&store, &record.worker.operation_id).is_err());
    }
    #[test]
    fn absent_journal_keeps_legacy_worker_storage_validation() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        assert_eq!(expected(&store, &record().worker).unwrap(), None);
        assert!(!retained(&store, &record().worker).unwrap());
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
            for _ in 0..2 {
                super::super::terminate(root.path(), &settings, &Cancellation::default()).unwrap();
                let store = Store::lock(root.path()).unwrap();
                let saved = store.load().unwrap().unwrap();
                assert_eq!(saved.stage, crate::cloud_runtime::Stage::Deleted);
                assert_eq!(saved.operation, CreateState::Prepared);
                assert!(crate::cloud_runtime::lifecycle::can_remove(&store, &saved).unwrap());
            }
        }
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
}
