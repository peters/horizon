//! Worker and workspace storage deletion.
use super::{Connection, Deployment, Error, Event, Result, Runner, Settings, Stage, Store, drop_replacement, storage};
use horizon_cloud::{Cancellation, CreateState, ImageSide, Worker, WorkerSpec, runpod::RunPod};

/// # Errors
/// Terminates only the persisted, identity-checked worker; never called on UI drop.
/// Each step is emitted as a deletion stage; the saved record moves straight to `Deleted`.
pub fn terminate(
    root: &std::path::Path,
    settings: &Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let store = Store::lock(root)?;
    let mut state = store.load()?.ok_or(Error::Invalid("No cloud deployment"))?;
    #[cfg(feature = "tailnet-node-cleanup")]
    if state.stage == Stage::Deleted
        && (state.operation != CreateState::Prepared || !state.tailnet_enrollment.is_empty())
    {
        emit(Event::Snapshot(Box::new(state.clone())));
        if state.tailnet_enrollment.cleanup_pending() {
            cancel.check()?;
        }
        return cleanup_deleted(&store, &mut state, emit);
    }
    if state.spec.is_none() {
        return Err(Error::Invalid("No worker was requested"));
    }
    crate::cloud_runtime::providers::lifecycle(&state, settings).delete(&store, &mut state, cancel, emit)?;
    finish_termination(state, &store, emit)
}

/// What this computer's own records prove about the provider resources of a cloud. It
/// sends no provider request and reads no credential: each provider records its own
/// confirmation around the request it sent, and this only reports it.
#[cfg(feature = "tailnet-node-cleanup")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Absence {
    /// The record and every provider journal confirm that nothing the cloud created
    /// remains at the provider.
    Proven,
    /// A provider journal still holds something the cloud created.
    MayRemain,
    /// No absent confirmation: there is no readable record, the deletion did not finish,
    /// or its provider journal is not settled yet.
    Unconfirmed,
}

/// Reads the cloud's records for what its provider confirmed.
/// # Errors
/// A record the caller cannot read now, such as one another operation holds, is not a
/// verdict and is reported as it is.
#[cfg(feature = "tailnet-node-cleanup")]
pub fn absence(root: &std::path::Path) -> Result<Absence> {
    let store = Store::lock_existing(root)?;
    let Some(state) = store.load()? else {
        return Ok(Absence::Unconfirmed);
    };
    if crate::cloud_runtime::providers::retained(&store, &state)? {
        return Ok(Absence::MayRemain);
    }
    Ok(
        if crate::cloud_runtime::providers::deletion_confirmed(&store, &state)? {
            Absence::Proven
        } else {
            Absence::Unconfirmed
        },
    )
}

fn finish_termination(state: Deployment, store: &Store, emit: &dyn Fn(Event)) -> Result<()> {
    let state = finish_deletion(state, store)?;
    // Publish the saved provider-deletion fact before node cleanup can fail and
    // before another controller can take the original deployment lock.
    emit(Event::Snapshot(Box::new(state.clone())));
    #[cfg(feature = "tailnet-node-cleanup")]
    {
        let mut state = state;
        cleanup_deleted(store, &mut state, emit)
    }
    #[cfg(not(feature = "tailnet-node-cleanup"))]
    Ok(())
}

#[cfg(feature = "tailnet-node-cleanup")]
fn cleanup_deleted(store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<()> {
    let result = super::super::tailnet::cleanup::retry(store, state, emit);
    // Cleanup changes the in-memory ledger only after its confirmation is saved.
    // Keep its last durable facts even if a later receipt or the caller's reload fails.
    emit(Event::Snapshot(Box::new(state.clone())));
    result
}

/// Deletes a Hetzner cloud's server, workspace volume and SSH key, each proven absent.
/// # Errors
/// Reports provider and persistence failures, leaving records a retry resumes from.
pub(in crate::cloud_runtime) fn delete_hetzner(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    deletion_step(
        emit,
        Stage::DeleteWorker,
        "Deleting the Hetzner server, workspace volume and SSH key and confirming their removal",
    );
    // Once deletion starts it runs to the end; each step is recorded so it can resume.
    cancel.check()?;
    super::hetzner::lifecycle::delete(store, state, settings, &committed())
}

/// Deletes an Azure cloud's resource group, which holds its VM, disks, address and
/// network, and proves the group and every resource tagged with the cloud absent.
/// # Errors
/// Reports provider and persistence failures, leaving records a retry resumes from.
pub(in crate::cloud_runtime) fn delete_azure(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let client = super::azure::client(settings, store)?;
    deletion_step(
        emit,
        Stage::DeleteWorker,
        "Deleting the Azure resource group and confirming its removal",
    );
    // Once deletion starts it runs to the end; each step is recorded so it can resume.
    cancel.check()?;
    super::azure::delete(&client, store, state, &committed())
}

/// Terminates a `RunPod` cloud's identity-checked worker, releasing its hosted devices
/// first, then deletes its managed workspace storage.
/// # Errors
/// Reports provider and persistence failures, leaving records a retry resumes from.
pub(in crate::cloud_runtime) fn delete_runpod(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
    emit: &dyn Fn(Event),
) -> Result<()> {
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
    let replacement = state.replacement_worker()?;
    let provider = RunPod::new(settings.credential()?);
    if state.operation != CreateState::Prepared {
        let runner = Runner {
            cancel,
            emit,
            secrets: Vec::new(),
        };
        terminate_worker(&provider, store, state, (spec, replacement), settings, &runner)?;
    }
    deletion_step(
        emit,
        Stage::DeleteStorage,
        "Deleting managed workspace storage and confirming its removal",
    );
    storage::terminate(&provider, store, state, &committed(), emit)
}

/// `specs` holds the recorded worker and, while a replacement is journaled, the
/// worker on its replacement image.
fn terminate_worker(
    provider: &RunPod,
    store: &Store,
    state: &mut Deployment,
    specs: (WorkerSpec, Option<WorkerSpec>),
    settings: &Settings,
    runner: &Runner<'_>,
) -> Result<()> {
    let cancel = runner.cancel;
    let release = state.requires_browserstack_release();
    let first = if release {
        Stage::ReleaseDevices
    } else {
        Stage::DeleteWorker
    };
    let (spec, replacement) = specs;
    let mut operation = state.operation.clone();
    if operation == CreateState::Requested {
        deletion_step(runner.emit, first, "Reconciling the requested worker");
        provider.ensure(
            &spec,
            &mut operation,
            cancel,
            |next| {
                state.operation = next.clone();
                store.save(state).map_err(|_| horizon_cloud::CloudError::Persistence)
            },
            |_| {},
        )?;
    }
    let spec = match (replacement, &operation) {
        (Some(next), CreateState::Bound { worker_id } | CreateState::Terminated { worker_id }) => {
            deletion_step(runner.emit, first, "Identifying which image the worker runs");
            reported_spec(provider.inspect(worker_id, cancel)?.as_ref(), spec, next)?
        }
        _ => spec,
    };
    if release && let CreateState::Bound { worker_id } = &operation {
        deletion_step(runner.emit, Stage::ReleaseDevices, "Confirming worker identity");
        let worker = provider.inspect(worker_id, cancel)?.ok_or(Error::Invalid(
            "Worker is lost; remote-device release must be verified before cleanup can be confirmed",
        ))?;
        worker.verify(&spec)?;
        if worker.status() == horizon_cloud::WorkerStatus::Stopped {
            return Err(Error::Invalid(
                "Resume the worker to release its hosted devices before deletion",
            ));
        }
        let connection = Connection::new(&worker, settings, store.root())?;
        crate::cloud_runtime::browser_auth::revoke(&connection, runner)?;
        state.browserstack_released = true;
        store.save(state)?;
    }
    let committed = commit_deletion(cancel)?;
    deletion_step(
        runner.emit,
        Stage::DeleteWorker,
        "Deleting the worker and confirming its removal",
    );
    provider.terminate_with_progress(
        &spec,
        &mut operation,
        &committed,
        |next| {
            state.operation = next.clone();
            store.save(state).map_err(|_| horizon_cloud::CloudError::Persistence)
        },
        request_detail(runner.emit),
    )?;
    Ok(())
}

/// The token for delete requests. A sent delete cannot be recalled, and a cancelled
/// confirmation would report an accepted delete as failed, so cancellation ends once
/// deletion starts; the provider timeout still bounds every request.
fn committed() -> Cancellation {
    Cancellation::default()
}

/// Ends the cancellable part of a worker deletion. A cancellation requested while
/// an earlier step was shown stops here, before the irreversible delete request.
fn commit_deletion(cancel: &Cancellation) -> Result<Cancellation> {
    cancel.check()?;
    Ok(committed())
}

fn deletion_step(emit: &dyn Fn(Event), stage: Stage, detail: &str) {
    emit(Event::stage(stage));
    emit(Event::Progress(crate::cloud_runtime::progress::Progress::activity(
        detail,
    )));
}

/// Replaces the current step's detail with each provider request before it is sent.
pub(super) fn request_detail(emit: &dyn Fn(Event)) -> impl FnMut(horizon_cloud::Progress) {
    move |request| {
        emit(Event::Progress(crate::cloud_runtime::progress::Progress::activity(
            request.to_string(),
        )));
    }
}

/// While a journaled replacement has a built image, the worker may run either image of
/// the pair. Delete it as the one it reports; the provider verifies that again strictly.
fn reported_spec(worker: Option<&Worker>, current: WorkerSpec, next: WorkerSpec) -> Result<WorkerSpec> {
    let side = worker.map(|worker| worker.verify_either(&current, &next)).transpose()?;
    Ok(if side == Some(ImageSide::Next) { next } else { current })
}

pub(super) fn finish_deletion(mut state: Deployment, store: &Store) -> Result<Deployment> {
    drop_replacement(store, &mut state)?;
    state.stage = Stage::Deleted;
    state.worker = None;
    store.save(&state)?;
    state.version = state.record_version();
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_deletion_commits_only_before_its_caller_cancels() {
        let cancel = Cancellation::default();
        let committed = commit_deletion(&cancel).unwrap();
        cancel.cancel();
        assert!(!committed.is_cancelled(), "a sent delete ignores later cancellation");
        assert!(matches!(
            commit_deletion(&cancel),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
    }
    #[test]
    fn deletion_identifies_a_replacing_worker_by_either_image() {
        let current: WorkerSpec = serde_json::from_value(serde_json::json!({
            "operation_id":"replacing","image_digest":format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8},
            "public_key":"unused","registry_auth_id":null,"gpu_types":[],"cpu_flavors":["cpu3c"],"data_centers":[]
        }))
        .unwrap();
        let mut next = current.clone();
        next.image_digest = format!("registry.example/worker@sha256:{}", "b".repeat(64));
        let worker = |image: &str, name: &str| -> Worker {
            serde_json::from_value(serde_json::json!({
                "id":"worker1","name":name,"imageName":image,"desiredStatus":"RUNNING",
                "env":{"HORIZON_CLOUD_OPERATION":"replacing"}
            }))
            .unwrap()
        };
        let name = current.name();
        for (reported, expected) in [
            (Some(worker(&current.image_digest, &name)), &current),
            (Some(worker(&next.image_digest, &name)), &next),
            (None, &current),
        ] {
            assert_eq!(
                reported_spec(reported.as_ref(), current.clone(), next.clone()).unwrap(),
                *expected
            );
        }
        let third = format!("registry.example/worker@sha256:{}", "c".repeat(64));
        for reported in [worker(&third, &name), worker(&next.image_digest, "horizon-cloud-other")] {
            assert!(reported_spec(Some(&reported), current.clone(), next.clone()).is_err());
        }
    }
    #[test]
    fn each_provider_deletion_request_becomes_the_current_detail() {
        use horizon_cloud::Progress;
        let details = std::cell::RefCell::new(Vec::new());
        let emit = |event: Event| {
            if let Event::Progress(progress) = event {
                details.borrow_mut().push(progress.detail);
            }
        };
        let mut forward = request_detail(&emit);
        for request in [
            Progress::ConfirmingWorker,
            Progress::Terminating,
            Progress::ConfirmingTermination,
            Progress::ConfirmingVolume,
            Progress::CheckingAttachments,
            Progress::InspectingMounts { worker: 2, workers: 3 },
            Progress::DeletingVolume,
            Progress::ConfirmingVolumeDeletion,
        ] {
            forward(request);
        }
        assert_eq!(
            details.take(),
            [
                "Confirming worker identity",
                "Requesting worker deletion",
                "Confirming worker removal",
                "Confirming workspace storage identity",
                "Checking workspace storage attachments",
                "Checking workspace storage attachments · worker 2 of 3",
                "Requesting workspace storage deletion",
                "Confirming workspace storage removal",
            ]
        );
    }

    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    fn snapshot_fixture(captured: bool) -> (tempfile::TempDir, Store, Deployment, Settings) {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::lock(&temp.path().join("cloud")).unwrap();
        let profile = serde_json::json!({
            "provider":"runpod", "image":"registry.example/worker", "cpu":4, "memory_gb":8
        });
        let mut state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1, "cloud_id":"cloud", "repository":"/synthetic", "revision":"a",
            "profile":profile, "stage":"Ready", "operation":{"state":"bound","worker_id":"worker1"},
            "spec":{"operation_id":"cloud", "image_digest":"sha256:synthetic", "profile":profile,
                "public_key":"synthetic", "registry_auth_id":null, "gpu_types":[], "cpu_flavors":[],
                "data_centers":[]},
            "worker":{"id":"worker1", "name":"cloud", "imageName":"registry.example/worker",
                "desiredStatus":"RUNNING"}, "sessions":[]
        }))
        .unwrap();
        if captured {
            state.tailnet_enrollment = serde_json::from_value(serde_json::json!({"receipts":[{
                "tailnet_id":"work", "provider":"runpod", "worker_id":"worker1",
                "allocation_request_id":"cloud", "device_id":"n-exact"
            }]}))
            .unwrap();
            std::fs::write(store.root().join("tailnet.json"), b"{").unwrap();
        } else {
            super::super::super::tailnet::Selection {
                tailnet: Some("work".into()),
            }
            .commit(store.root())
            .unwrap();
        }
        store.save(&state).unwrap();
        state = store.load().unwrap().unwrap();
        let settings = serde_json::from_value(serde_json::json!({
            "runpod_key_file":temp.path().join("absent-key"),
            "ssh_identity_file":temp.path().join("absent-identity"),
            "docker_config":temp.path().join("absent-docker"),
            "registry_pull_auth_id":null, "cpu_flavors":[], "gpu_types":[]
        }))
        .unwrap();
        (temp, store, state, settings)
    }

    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    #[test]
    fn deletion_snapshot_is_durable_and_locked_before_node_cleanup_can_fail() {
        for captured in [false, true] {
            let (_temp, store, mut state, _settings) = snapshot_fixture(captured);
            state.stage = Stage::DeleteStorage;
            state.operation = CreateState::Terminated {
                worker_id: "worker1".into(),
            };
            let selection = std::fs::read(store.root().join("tailnet.json")).unwrap();
            let snapshots = std::cell::RefCell::new(Vec::new());
            let emit = |event| {
                let Event::Snapshot(snapshot) = event else {
                    panic!("only the durable snapshot precedes this local cleanup failure")
                };
                assert!(matches!(Store::lock(store.root()), Err(Error::Busy)));
                let bytes = std::fs::read(store.root().join("deployment.json")).unwrap();
                assert_eq!(bytes, serde_json::to_vec_pretty(&snapshot).unwrap());
                assert_eq!(snapshot.stage, Stage::Deleted);
                assert!(snapshot.worker.is_none());
                assert_eq!(snapshot.operation, state.operation);
                assert_eq!(snapshot.tailnet_enrollment, state.tailnet_enrollment);
                snapshots.borrow_mut().push(snapshot);
            };
            let error = finish_termination(state.clone(), &store, &emit).unwrap_err();
            assert!(error.to_string().contains(if captured {
                "Tailnet settings or OS credential are unavailable"
            } else {
                "not recorded"
            }));
            assert_eq!(snapshots.borrow().len(), 2);
            assert_eq!(std::fs::read(store.root().join("tailnet.json")).unwrap(), selection);
            assert_eq!(
                serde_json::to_value(store.load().unwrap().unwrap()).unwrap(),
                serde_json::to_value(snapshots.borrow()[0].as_ref()).unwrap()
            );
        }
    }

    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    #[test]
    fn failed_deletion_save_never_publishes_a_deleted_snapshot() {
        let (_temp, store, mut state, _settings) = snapshot_fixture(true);
        let original = std::fs::read(store.root().join("deployment.json")).unwrap();
        state.version = u32::MAX;
        assert!(finish_termination(state, &store, &|_| panic!("failed save is no deletion fact")).is_err());
        assert_eq!(std::fs::read(store.root().join("deployment.json")).unwrap(), original);
        assert_eq!(store.load().unwrap().unwrap().stage, Stage::Ready);
    }

    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    #[test]
    fn deleted_retry_snapshot_precedes_cancellation_or_legacy_cleanup_failure() {
        for captured in [false, true] {
            let (_temp, store, mut state, settings) = snapshot_fixture(captured);
            state.operation = CreateState::Terminated {
                worker_id: "worker1".into(),
            };
            let state = finish_deletion(state, &store).unwrap();
            let original = std::fs::read(store.root().join("deployment.json")).unwrap();
            let root = store.root().to_path_buf();
            drop(store);
            let cancel = Cancellation::default();
            if captured {
                cancel.cancel();
            }
            let observed = std::cell::Cell::new(0);
            let emit = |event| {
                let Event::Snapshot(snapshot) = event else {
                    panic!("snapshot must come first")
                };
                assert!(matches!(Store::lock(&root), Err(Error::Busy)));
                assert_eq!(serde_json::to_vec_pretty(&snapshot).unwrap(), original);
                assert_eq!(snapshot.stage, state.stage);
                observed.set(observed.get() + 1);
            };
            let error = terminate(&root, &settings, &cancel, &emit).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(if captured { "cancelled" } else { "not recorded" })
            );
            assert_eq!(observed.get(), if captured { 1 } else { 2 });
            assert_eq!(std::fs::read(root.join("deployment.json")).unwrap(), original);
            assert!(Store::lock(&root).is_ok(), "the original lock ends with its controller");
        }
    }

    #[cfg(all(unix, feature = "tailnet-node-cleanup"))]
    #[test]
    fn confirmed_cleanup_publishes_its_final_snapshot_under_the_original_lock() {
        let (_temp, store, mut state, settings) = snapshot_fixture(true);
        super::super::super::tailnet::Selection {
            tailnet: Some("work".into()),
        }
        .commit(store.root())
        .unwrap();
        let receipt = state.tailnet_enrollment.receipts()[0].clone();
        state.tailnet_enrollment.confirm_removed(&receipt).unwrap();
        state.operation = CreateState::Terminated {
            worker_id: "worker1".into(),
        };
        let observed = std::cell::Cell::new(0);
        let emit = |event| {
            let Event::Snapshot(snapshot) = event else {
                panic!("confirmed receipt must not reach Node API")
            };
            assert!(matches!(Store::lock(store.root()), Err(Error::Busy)));
            assert_eq!(snapshot.stage, Stage::Deleted);
            assert!(snapshot.tailnet_enrollment.receipts()[0].node_deleted);
            assert_eq!(
                serde_json::to_vec_pretty(&snapshot).unwrap(),
                std::fs::read(store.root().join("deployment.json")).unwrap()
            );
            observed.set(observed.get() + 1);
        };
        finish_termination(state, &store, &emit).unwrap();
        assert_eq!(observed.get(), 2);
        let root = store.root().to_path_buf();
        drop(store);
        let retries = std::cell::Cell::new(0);
        terminate(&root, &settings, &Cancellation::default(), &|event| {
            let Event::Snapshot(snapshot) = event else {
                panic!("confirmed retry performs no cleanup request")
            };
            assert!(matches!(Store::lock(&root), Err(Error::Busy)));
            assert!(snapshot.tailnet_enrollment.receipts()[0].node_deleted);
            retries.set(retries.get() + 1);
        })
        .unwrap();
        assert_eq!(retries.get(), 2);
    }
}
