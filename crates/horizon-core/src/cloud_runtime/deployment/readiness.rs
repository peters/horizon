//! One deadline for provider inspection, SSH probes and retry delays.
use super::{Connection, Error, Event, Request, Result, Runner, Stage, Store};
use horizon_cloud::{Cancellation, WorkerSpec, runpod::RunPod};
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
    poll(&deadline, runner.cancel, |deadline| {
        let id = &state
            .worker
            .as_ref()
            .ok_or(Error::Invalid("Missing worker identity"))?
            .id;
        let inspected = provider.inspect_with_timeout(id, runner.cancel, deadline.remaining(runner.cancel)?);
        deadline.remaining(runner.cancel)?;
        let worker = inspected?.ok_or(horizon_cloud::CloudError::WorkerLost)?;
        worker.verify(spec)?;
        worker.verify_resources(spec)?;
        let connection = Connection::new(&worker, &request.settings, store.root());
        state.worker = Some(worker);
        store.save(state)?;
        if let Ok(connection) = connection {
            (runner.emit)(Event::Progress(super::super::progress::Progress::activity(
                "Waiting for SSH and worker services",
            )));
            if connection
                .ready(runner, &state.profile.capabilities, deadline.remaining(runner.cancel)?)
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
