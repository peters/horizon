//! Stops that Horizon did not make: a worker that stopped itself, as an agent's stop
//! does, or that was stopped through the provider account. Horizon learns of such a
//! stop when its connection to a ready worker fails, and then asks the provider.
use super::{ReconciledDeployment, reconcile_locked};
use crate::cloud_runtime::{
    CreateState, Error, Result, Stage,
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation,
    provider::{Description, StoppedCost},
};
use std::{path::Path, time::Duration};

/// How often a provider check is tried, a second apart, while another operation
/// holds the cloud. Longer than the provider check it may wait for: a cancelled check
/// can still finish its settle and inspect requests, each bounded by the provider's
/// request timeout.
const BUSY_ATTEMPTS: u64 = 3 * horizon_cloud::runpod::REQUEST_TIMEOUT.as_secs();

/// Whether a stop that Horizon did not make can explain a failed connection to the
/// cloud `state` records: a bound worker that Horizon does not stop, replace or
/// delete itself, on a provider whose stop keeps the worker. Only such a worker can
/// stop without Horizon; a Hetzner stop is Horizon's own release of the server.
#[must_use]
pub fn may_stop_outside(state: &Deployment) -> bool {
    Description::of(&state.profile).stopped == StoppedCost::WorkerKept
        && matches!(state.operation, CreateState::Bound { .. })
        && !matches!(state.stage, Stage::Stopping | Stage::Replace | Stage::Deleted)
}

/// Asks the provider, as Check provider does, about the worker of a cloud whose
/// connection failed. A stop the provider confirms is recorded as an ordinary
/// stopped cloud, so only Resume starts the worker again. The check waits while
/// another operation, such as another check of the same worker, holds the cloud.
///
/// Returns `None`, without asking the provider, when [`may_stop_outside`] refuses
/// the record.
/// # Errors
/// Reports a provider check that failed, or a cloud that another operation holds.
pub fn check_lost_worker(
    root: &Path,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<Option<ReconciledDeployment>> {
    check_with(root, cancel, Duration::from_secs(1), |store| {
        reconcile_locked(store, settings, None, cancel)
    })
}

fn check_with(
    root: &Path,
    cancel: &Cancellation,
    pause: Duration,
    check: impl Fn(&Store) -> Result<ReconciledDeployment>,
) -> Result<Option<ReconciledDeployment>> {
    for attempt in 0..BUSY_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(pause);
        }
        // A cancelled check never takes the cloud, so it cannot hold it from the
        // check or operation that cancelled it.
        cancel.check()?;
        let store = match Store::lock(root) {
            Err(Error::Busy) => continue,
            other => other?,
        };
        if !store.load()?.as_ref().is_some_and(may_stop_outside) {
            return Ok(None);
        }
        return check(&store).map(Some);
    }
    Err(Error::Busy)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use horizon_cloud::{
        Worker,
        runpod::recovery::{Outcome, Reconciliation},
    };

    fn save(root: &Path, recorded: &str, provider: &str) -> Deployment {
        let profile = serde_json::json!({"provider":provider,"image":"registry.example/worker","cpu":2,
            "memory_gb":4});
        let state: Deployment = serde_json::from_value(serde_json::json!({
            "version":1,"cloud_id":"lost-cloud","repository":"/synthetic","revision":"a".repeat(40),
            "profile":profile,"stage":recorded,"operation":{"state":"bound","worker_id":"worker1"},
            "spec":null,"sessions":[],"source_ready":true,
            "worker":{"id":"worker1","name":"lost-cloud","imageName":"registry.example/worker",
                "desiredStatus":"RUNNING","publicIp":"192.0.2.10","portMappings":{"22":40022}}
        }))
        .unwrap();
        Store::lock(root).unwrap().save(&state).unwrap();
        state
    }

    /// What the provider check reports for the saved record: the worker `status`,
    /// recorded as stopped when it is.
    fn reported(status: &'static str) -> impl Fn(&Store) -> Result<ReconciledDeployment> {
        move |store| {
            let mut state = store.load()?.unwrap();
            let worker: Worker = serde_json::from_value(serde_json::json!({
                "id":"worker1","name":"lost-cloud","imageName":"registry.example/worker","desiredStatus":status
            }))
            .unwrap();
            if status == "EXITED" {
                state.stage = Stage::Stopped;
                state.stop_requested = true;
            }
            state.worker = Some(worker.clone());
            store.save(&state)?;
            Ok(ReconciledDeployment {
                state,
                report: Reconciliation {
                    operation_id: "lost-cloud".into(),
                    outcome: Outcome::Inactive {
                        worker_id: "worker1".into(),
                    },
                    worker: Some(worker),
                },
            })
        }
    }

    /// The saved record of a stop the provider confirmed, with the provider check given.
    fn stopped_with(
        root: &Path,
        cancel: &Cancellation,
        check: impl Fn(&Store) -> Result<ReconciledDeployment>,
    ) -> Result<Option<Deployment>> {
        Ok(check_with(root, cancel, Duration::ZERO, check)?
            .filter(ReconciledDeployment::confirmed_stopped)
            .map(|reconciled| reconciled.state))
    }

    #[test]
    fn only_a_stop_the_provider_confirms_is_reported_and_recorded() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        save(temp.path(), "Ready", "runpod");
        let running = check_with(temp.path(), &cancel, Duration::ZERO, reported("RUNNING"))
            .unwrap()
            .unwrap();
        assert!(!running.confirmed_stopped(), "a running worker keeps the failure");
        assert!(
            stopped_with(temp.path(), &cancel, reported("RUNNING"))
                .unwrap()
                .is_none()
        );
        let exited = stopped_with(temp.path(), &cancel, reported("EXITED")).unwrap().unwrap();
        assert_eq!(exited.stage, Stage::Stopped);
        let saved = Store::lock(temp.path()).unwrap().load().unwrap().unwrap();
        assert!(
            saved.stop_requested && saved.stage == Stage::Stopped,
            "only Resume starts it"
        );
    }

    #[test]
    fn a_stop_horizon_makes_itself_or_a_foreign_record_is_not_checked() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let unasked = |_: &Store| -> Result<ReconciledDeployment> { panic!("the provider is not asked") };
        for stage in ["Stopping", "Replace", "Deleted"] {
            save(temp.path(), stage, "runpod");
            assert!(
                stopped_with(temp.path(), &cancel, unasked).unwrap().is_none(),
                "{stage}"
            );
        }
        let mut requested = save(temp.path(), "Provision", "runpod");
        requested.operation = CreateState::Requested;
        Store::lock(temp.path()).unwrap().save(&requested).unwrap();
        assert!(stopped_with(temp.path(), &cancel, unasked).unwrap().is_none());
        // A Hetzner stop releases the server; only Horizon makes it.
        save(temp.path(), "Ready", "hetzner");
        assert!(stopped_with(temp.path(), &cancel, unasked).unwrap().is_none());
    }

    #[test]
    fn a_held_cloud_is_checked_again_and_then_reported_busy() {
        let temp = tempfile::tempdir().unwrap();
        save(temp.path(), "Ready", "runpod");
        let held = Store::lock(temp.path()).unwrap();
        let result = stopped_with(temp.path(), &Cancellation::default(), reported("EXITED"));
        assert!(matches!(result, Err(Error::Busy)), "{result:?}");
        drop(held);
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let held = Store::lock(temp.path()).unwrap();
        let result = stopped_with(temp.path(), &cancelled, reported("EXITED"));
        assert!(matches!(result, Err(Error::Provider(_))), "{result:?}");
        drop(held);
        // Even with the cloud free, a cancelled check neither takes it nor asks the provider.
        let unasked = |_: &Store| -> Result<ReconciledDeployment> { panic!("the provider is not asked") };
        assert!(matches!(
            stopped_with(temp.path(), &cancelled, unasked),
            Err(Error::Provider(_))
        ));
        assert!(Store::lock(temp.path()).is_ok(), "the cloud stays free");
    }
}
