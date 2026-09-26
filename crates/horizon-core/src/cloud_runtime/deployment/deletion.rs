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
    let spec = state.spec.clone().ok_or(Error::Invalid("No worker was requested"))?;
    let replacement = state.replacement_worker()?;
    let provider = RunPod::new(settings.credential()?);
    if state.operation != CreateState::Prepared {
        let runner = Runner {
            cancel,
            emit,
            secrets: Vec::new(),
        };
        terminate_worker(&provider, &store, &mut state, (spec, replacement), settings, &runner)?;
    }
    deletion_step(
        emit,
        Stage::DeleteStorage,
        "Deleting managed workspace storage and confirming its removal",
    );
    storage::terminate(&provider, &store, &state, &committed(), emit)?;
    finish_deletion(state, &store)
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

pub(super) fn finish_deletion(mut state: Deployment, store: &Store) -> Result<()> {
    drop_replacement(store, &mut state)?;
    state.stage = Stage::Deleted;
    state.worker = None;
    store.save(&state)
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
}
