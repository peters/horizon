//! One deadline for provider inspection, SSH probes and retry delays.
use super::{Connection, Error, Event, Request, Result, Runner, Stage, Store};
use horizon_cloud::{Cancellation, Worker, WorkerSpec, runpod::RunPod};
use std::time::{Duration, Instant};

const TIMEOUT: &str = "Worker readiness timed out; worker remains allocated for inspection or explicit deletion";
struct Deadline(Instant);
impl Deadline {
    fn remaining(&self, cancel: &Cancellation) -> Result<Duration> {
        cancel.check()?;
        self.0
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
            .ok_or(Error::Invalid(TIMEOUT))
    }
}

pub(super) fn wait(
    request: &Request,
    provider: &RunPod,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut super::Deployment,
    spec: &WorkerSpec,
) -> Result<Connection> {
    let deadline = Deadline(Instant::now() + Duration::from_secs(u64::from(state.profile.bootstrap.readiness_seconds)));
    state.stage = Stage::Readiness;
    store.save(state)?;
    (runner.emit)(Event::stage(state.stage));
    let capabilities = state.profile.capabilities.clone();
    poll(&deadline, runner.cancel, |deadline| {
        let id = &state
            .worker
            .as_ref()
            .ok_or(Error::Invalid("Missing worker identity"))?
            .id;
        let inspected = provider.inspect_with_timeout(id, runner.cancel, deadline.remaining(runner.cancel)?);
        deadline.remaining(runner.cancel)?;
        let worker = inspected?.ok_or(horizon_cloud::CloudError::WorkerLost)?;
        with_verified_worker(worker, spec, store, state, |worker| {
            if let Ok(connection) = Connection::new(worker, &request.settings, store.root()) {
                (runner.emit)(Event::Progress(super::super::progress::Progress::activity(
                    "Waiting for SSH and worker services",
                )));
                if connection
                    .ready(runner, &capabilities, deadline.remaining(runner.cancel)?)
                    .is_ok()
                {
                    return Ok(Some(connection));
                }
            } else {
                (runner.emit)(Event::Progress(super::super::progress::Progress::activity(
                    "Waiting for the provider to publish an SSH endpoint",
                )));
            }
            Ok(None)
        })
    })
}

fn with_verified_worker<T>(
    worker: Worker,
    spec: &WorkerSpec,
    store: &Store,
    state: &mut super::Deployment,
    ready: impl FnOnce(&Worker) -> Result<T>,
) -> Result<T> {
    worker.verify(spec)?;
    state.worker = Some(worker);
    store.save(state)?;
    let worker = state.worker.as_ref().ok_or(Error::Invalid("Missing worker identity"))?;
    worker.verify_resources(spec)?;
    ready(worker)
}

fn poll<T>(
    deadline: &Deadline,
    cancel: &Cancellation,
    mut probe: impl FnMut(&Deadline) -> Result<Option<T>>,
) -> Result<T> {
    loop {
        deadline.remaining(cancel)?;
        let ready = probe(deadline)?;
        deadline.remaining(cancel)?;
        if let Some(ready) = ready {
            return Ok(ready);
        }
        for _ in 0..20 {
            std::thread::sleep(deadline.remaining(cancel)?.min(Duration::from_millis(100)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn rejected_inspected_storage_is_durable_before_any_ssh_probe() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::lock(root.path()).unwrap();
        let mut state: super::super::Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"storage","repository":"/fixture","revision":"a",
            "profile":{"provider":"runpod","image":"registry.example/worker","cpu":4,"memory_gb":8,"gpu":false},
            "stage":"Readiness","operation":{"state":"bound","worker_id":"worker1"},"spec":null,
            "worker":null,"source_ready":true,"ready_history":"Observed","ready_after_seconds":42,
            "sessions":[{"panel_id":"panel1","agent":"claude","tmux":"session1","branch":"fixture","worktree":"/workspace/fixture"}]
        }))
        .unwrap();
        let spec = WorkerSpec {
            operation_id: state.cloud_id.clone(),
            image_digest: format!("registry.example/worker@sha256:{}", "a".repeat(64)),
            profile: state.profile.clone(),
            public_key: String::new(),
            registry_auth_id: None,
            gpu_types: Vec::new(),
            cpu_flavors: vec!["cpu3g".into()],
            data_centers: Vec::new(),
            network_volume: None,
        };
        state.spec = Some(spec.clone());
        let mut inspected = serde_json::json!({
            "id":"worker1","name":spec.name(),"imageName":spec.image_digest,"desiredStatus":"RUNNING",
            "vcpuCount":4,"memoryInGb":8,"containerDiskInGb":20,"volumeInGb":20,
            "volumeMountPath":"/workspace","publicIp":"192.0.2.1","portMappings":{"22":22001}
        });
        state.worker = Some(serde_json::from_value(inspected.clone()).unwrap());
        store.save(&state).unwrap();
        inspected["volumeInGb"] = serde_json::json!(0);
        inspected["portMappings"]["22"] = serde_json::json!(22002);
        let result = with_verified_worker::<()>(
            serde_json::from_value(inspected.clone()).unwrap(),
            &spec,
            &store,
            &mut state,
            |_| panic!("SSH readiness must not run for rejected storage"),
        );
        assert!(matches!(
            result,
            Err(Error::Provider(horizon_cloud::CloudError::Invalid(_)))
        ));
        let restored = store.load().unwrap().unwrap();
        assert_eq!(restored.worker.as_ref().unwrap().volume_in_gb, Some(0));
        assert_eq!(restored.worker.as_ref().unwrap().ssh_address().unwrap().port(), 22002);
        assert_eq!(restored.operation, state.operation);
        assert_eq!(restored.stage, Stage::Readiness);
        assert!(restored.source_ready);
        assert_eq!(restored.ready_after_seconds, Some(42));
        assert_eq!(restored.ready_history, super::super::ReadyHistory::Observed);
        assert_eq!(restored.sessions[0].worktree, "/workspace/fixture");

        inspected["volumeInGb"] = serde_json::json!(20);
        let called = std::cell::Cell::new(false);
        with_verified_worker(
            serde_json::from_value(inspected).unwrap(),
            &spec,
            &store,
            &mut state,
            |_| {
                assert_eq!(store.load().unwrap().unwrap().worker.unwrap().volume_in_gb, Some(20));
                called.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(called.get());
    }

    #[test]
    fn expired_and_cancelled_readiness_never_start_another_probe() {
        let cancel = Cancellation::default();
        let expired = Deadline(Instant::now());
        assert!(poll::<()>(&expired, &cancel, |_| panic!("expired probe")).is_err());
        cancel.cancel();
        assert!(matches!(
            poll::<()>(&Deadline(Instant::now() + Duration::from_secs(1)), &cancel, |_| panic!(
                "cancelled probe"
            )),
            Err(Error::Provider(horizon_cloud::CloudError::Cancelled))
        ));
    }
    #[test]
    fn probe_and_retry_sleep_share_one_deadline_and_late_success_is_rejected() {
        for ready in [false, true] {
            let cancel = Cancellation::default();
            let deadline = Deadline(Instant::now() + Duration::from_millis(30));
            let mut probes = 0;
            let result = poll(&deadline, &cancel, |deadline| {
                probes += 1;
                let before = deadline.remaining(&cancel)?;
                std::thread::sleep(if ready {
                    before + Duration::from_millis(10)
                } else {
                    before / 2
                });
                Ok(ready.then_some(()))
            });
            assert!(matches!(result, Err(Error::Invalid(TIMEOUT))));
            assert_eq!(probes, 1);
        }
    }
}
