//! Idle stop for Hetzner clouds. A Hetzner worker holds no credential that could
//! stop its billing, so its idle watcher only records how long it has been idle;
//! Horizon reads that record over SSH and releases the server as Stop does,
//! keeping the workspace volume. Only a running Horizon can do this.
use super::{Compute, lifecycle::stop_with};
use crate::cloud_runtime::{
    Error, Result, Stage,
    command::Runner,
    settings::Settings,
    ssh::Connection,
    state::{Deployment, Store},
};
use horizon_cloud::{Cancellation, CreateState, Worker};
use serde::Deserialize;
use std::{path::Path, time::Duration};

/// Prints the worker's idle record; see `horizon-worker-idle --report`.
const REPORT_COMMAND: &str = "horizon-worker-idle --report";
const REPORT_TIMEOUT: Duration = Duration::from_secs(30);

/// What one idle check found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleCheck {
    /// The cloud has no idle period, or is not a running cloud to check.
    NotWatched,
    /// Idle for `idle` of its `limit`, so it keeps running.
    Active { idle: Duration, limit: Duration },
    /// Idle for its whole period: the server was released and the volume kept.
    Stopped { idle: Duration },
}

/// The worker's idle record, as `horizon-worker-idle --report` prints it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cloud_runtime) struct Report {
    pub(in crate::cloud_runtime) idle_seconds: u64,
    pub(in crate::cloud_runtime) idle_stop_seconds: u64,
}

impl Report {
    /// # Errors
    /// Refuses output that is not one idle record.
    pub(in crate::cloud_runtime) fn parse(output: &str) -> Result<Self> {
        serde_json::from_str(output.trim()).map_err(|_| Error::Invalid("The worker's idle record is malformed"))
    }
}

/// The running cloud a check reads, as recorded when the check began.
struct Watched {
    worker_id: String,
    worker: Worker,
    limit: Duration,
}

impl Watched {
    /// Whether `other` is the same cloud on the same server with the same period.
    fn same(&self, other: &Self) -> bool {
        self.worker_id == other.worker_id
            && self.limit == other.limit
            && serde_json::to_value(&self.worker).ok() == serde_json::to_value(&other.worker).ok()
    }
}

/// The cloud `state` describes, when it is a running Hetzner cloud with an idle
/// period and no image replacement in progress.
fn watched(state: &Deployment) -> Option<Watched> {
    if state.profile.provider != horizon_cloud::hetzner::PROVIDER || state.refuse_pending_replacement().is_err() {
        return None;
    }
    let minutes = state.profile.idle_stop_minutes?;
    let CreateState::Bound { worker_id } = &state.operation else {
        return None;
    };
    (state.stage == Stage::Ready && !state.stop_requested).then_some(())?;
    Some(Watched {
        worker_id: worker_id.clone(),
        worker: state.worker.clone()?,
        limit: Duration::from_secs(u64::from(minutes) * 60),
    })
}

/// Reads the idle record of the cloud at `root` and stops it once it has been idle
/// for its whole period. The record is read without holding the cloud's lock, so
/// the stop proceeds only if the cloud is still the one that was read.
/// # Errors
/// Reports an unreadable, stale or foreign record, a busy cloud and a failed stop.
pub(in crate::cloud_runtime) fn check(root: &Path, settings: &Settings, cancel: &Cancellation) -> Result<IdleCheck> {
    check_with(
        root,
        cancel,
        |worker| read(worker, settings, root, cancel, REPORT_COMMAND),
        || Compute::cleanup(settings),
    )
}

/// As `check`, with the record reader and the Hetzner client given.
pub(super) fn check_with(
    root: &Path,
    cancel: &Cancellation,
    read: impl FnOnce(&Worker) -> Result<String>,
    compute: impl FnOnce() -> Result<Compute>,
) -> Result<IdleCheck> {
    let Some(before) = Store::lock(root)?.load()?.as_ref().and_then(watched) else {
        return Ok(IdleCheck::NotWatched);
    };
    let report = Report::parse(&read(&before.worker)?)?;
    // A worker started with another period would be stopped at the wrong time.
    if report.idle_stop_seconds != before.limit.as_secs() {
        return Err(Error::Invalid(
            "The worker's idle period is not this cloud's; redeploy it to apply the profile's idle_stop_minutes",
        ));
    }
    let idle = Duration::from_secs(report.idle_seconds);
    if idle < before.limit {
        return Ok(IdleCheck::Active {
            idle,
            limit: before.limit,
        });
    }
    let store = Store::lock(root)?;
    let Some(mut state) = store.load()? else {
        return Ok(IdleCheck::NotWatched);
    };
    // Stopped, resumed onto a new server or changed while the record was read.
    if !watched(&state).is_some_and(|now| now.same(&before)) {
        return Ok(IdleCheck::NotWatched);
    }
    stop_with(&compute()?, &store, &mut state, cancel)?;
    Ok(IdleCheck::Stopped { idle })
}

/// Runs `command`, which prints the idle record, on `worker` over its pinned connection.
pub(in crate::cloud_runtime) fn read(
    worker: &Worker,
    settings: &Settings,
    root: &Path,
    cancel: &Cancellation,
    command: &str,
) -> Result<String> {
    let connection = Connection::new(worker, settings, root)?;
    Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
    .run(
        "Reading the worker's idle record",
        &mut connection.pinned_command(command),
        REPORT_TIMEOUT,
    )
}
