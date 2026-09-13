//! Single-dispatch compute resume of an owned persistent Pod, never task replay.
use super::{
    ApiPod, InteractiveWorkerLifetime, RunPodError, RunPodInteractiveWorkerProvider, RunPodLifecycle, RunPodWorker,
    RunPodWorkerStatus, interactive::runpod_worker, status_from_resource, stop::RetainedMount, validate_target,
};
use crate::cloud_run::{
    interactive_worker::{InteractiveWorker, InteractiveWorkerSshEndpoint, InteractiveWorkerStatus},
    interactive_worker_start::{InteractiveWorkerStart, InteractiveWorkerStartProvider},
};
use std::time::Duration;

const TRANSITION_BOUND: Duration = Duration::from_secs(300);
const OBSERVATION_BACKOFF_MS: [u64; 6] = [0, 1_000, 2_000, 4_000, 8_000, 15_000];
const REPEATED_BACKOFF_MS: u64 = 30_000;

trait Clock {
    fn elapsed(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

#[cfg(not(test))]
struct MonotonicClock(std::time::Instant);

#[cfg(not(test))]
impl Clock for MonotonicClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

struct Snapshot {
    status: RunPodWorkerStatus,
    mount: RetainedMount,
}

impl RunPodInteractiveWorkerProvider {
    fn start_status(
        &self,
        status: RunPodWorkerStatus,
        worker: &InteractiveWorker,
        pin: &InteractiveWorkerSshEndpoint,
    ) -> Result<InteractiveWorkerStatus, RunPodError> {
        let status = self.adapt_status(status, &worker.target, &worker.ssh_public_key);
        if status.ssh.as_ref().is_some_and(|ssh| ssh != pin) {
            return Err(RunPodError::ResourceIdentityMismatch);
        }
        Ok(status)
    }

    fn start_snapshot(
        &self,
        pod: &ApiPod,
        worker: &RunPodWorker,
        expected: &InteractiveWorker,
        pin: &InteractiveWorkerSshEndpoint,
    ) -> Result<Snapshot, RunPodError> {
        let status = status_from_resource(pod, worker, Some(&expected.ssh_public_key))?;
        let mount = self
            .client
            .retained_mount(pod, &self.profile)
            .map_err(|_| RunPodError::StartUnverified)?;
        let metadata = &pod.stop;
        let valid = match status.lifecycle {
            RunPodLifecycle::Exited => {
                metadata.runtime.as_ref().is_some_and(serde_json::Value::is_null)
                    && metadata.locked == false
                    && metadata
                        .actions
                        .as_array()
                        .is_some_and(|actions| actions.iter().any(|v| v == "start"))
            }
            RunPodLifecycle::Running => metadata.runtime.as_ref().is_some_and(serde_json::Value::is_object),
            RunPodLifecycle::Provisioning => {
                pod.status.as_deref() == Some("STARTING")
                    && metadata.runtime.as_ref().is_some_and(serde_json::Value::is_null)
            }
            _ => false,
        };
        if !valid {
            return Err(RunPodError::StartUnverified);
        }
        if let Some(ssh) = pod.ssh.as_ref().and_then(|ssh| ssh.direct.as_ref())
            && (ssh.host != pin.host || ssh.port != pin.port || ssh.username != pin.username)
        {
            return Err(RunPodError::ResourceIdentityMismatch);
        }
        Ok(Snapshot { status, mount })
    }
}

impl RunPodInteractiveWorkerProvider {
    fn start_with_clock(
        &self,
        worker: &InteractiveWorker,
        clock: &impl Clock,
    ) -> Result<InteractiveWorkerStart, RunPodError> {
        self.client.check_network_worker(worker)?;
        let retained = runpod_worker(worker)?;
        validate_target(&worker.target, &self.profile)?;
        if worker.lifetime != InteractiveWorkerLifetime::Persistent {
            return Err(RunPodError::StartIdentityRequired);
        }
        let pin = self
            .host_keys
            .retained_endpoint(worker)
            .filter(InteractiveWorkerSshEndpoint::is_complete)
            .ok_or(RunPodError::StartIdentityRequired)?;
        if self.client.network_binding.is_none() && self.profile.volume_gib == 0 {
            return Err(RunPodError::StartUnverified);
        }
        let Some(pod) = self.client.transport.get(&retained.pod_id)? else {
            return Ok(InteractiveWorkerStart::AlreadyAbsent);
        };
        let before = self.start_snapshot(&pod, &retained, worker, &pin)?;
        if clock.elapsed() >= TRANSITION_BOUND {
            return Err(RunPodError::StartUnverified);
        }
        if before.status.lifecycle == RunPodLifecycle::Running {
            return self
                .start_status(before.status, worker, &pin)
                .map(InteractiveWorkerStart::AlreadyRunning);
        }
        // A STARTING Pod is only observed. An ambiguous POST response is never
        // authority to submit again; only independently observed state can succeed.
        if before.status.lifecycle == RunPodLifecycle::Exited {
            if TRANSITION_BOUND.saturating_sub(clock.elapsed()) < super::http::REQUEST_TIMEOUT {
                return Err(RunPodError::StartUnverified);
            }
            let _response = self.client.transport.start(&retained.pod_id);
        }
        self.await_running(worker, &retained, &pin, &before, clock)
    }

    fn await_running(
        &self,
        worker: &InteractiveWorker,
        retained: &RunPodWorker,
        pin: &InteractiveWorkerSshEndpoint,
        before: &Snapshot,
        clock: &impl Clock,
    ) -> Result<InteractiveWorkerStart, RunPodError> {
        // Reserve the complete existing per-request timeout for both Pod and, when
        // selected, volume reads. Never start an observation that cannot fit; the
        // final poll follows a clipped sleep and is not repeated in a busy loop.
        let reserve = super::http::REQUEST_TIMEOUT * if self.client.network_binding.is_some() { 2 } else { 1 };
        let final_window = reserve + Duration::from_secs(1);
        for delay_ms in OBSERVATION_BACKOFF_MS
            .into_iter()
            .chain(std::iter::repeat(REPEATED_BACKOFF_MS))
        {
            let remaining = TRANSITION_BOUND.saturating_sub(clock.elapsed());
            if remaining < reserve {
                break;
            }
            clock.sleep(Duration::from_millis(delay_ms).min(remaining.saturating_sub(final_window)));
            let remaining = TRANSITION_BOUND.saturating_sub(clock.elapsed());
            if remaining < reserve {
                break;
            }
            let final_poll = remaining <= final_window;
            let Some(pod) = self.client.transport.get(&retained.pod_id)? else {
                return Err(RunPodError::StartUnverified);
            };
            let current = self.start_snapshot(&pod, retained, worker, pin)?;
            if clock.elapsed() > TRANSITION_BOUND || current.mount != before.mount {
                return Err(RunPodError::StartUnverified);
            }
            if current.status.lifecycle == RunPodLifecycle::Running {
                return self
                    .start_status(current.status, worker, pin)
                    .map(InteractiveWorkerStart::Started);
            }
            if final_poll {
                break;
            }
        }
        Err(RunPodError::StartUnverified)
    }
}

impl InteractiveWorkerStartProvider for RunPodInteractiveWorkerProvider {
    fn start_worker(&self, worker: &InteractiveWorker) -> Result<InteractiveWorkerStart, Self::Error> {
        #[cfg(not(test))]
        let clock = MonotonicClock(std::time::Instant::now());
        #[cfg(test)]
        let clock = tests::TestClock::default();
        self.start_with_clock(worker, &clock)
    }
}

#[cfg(test)]
mod tests;
