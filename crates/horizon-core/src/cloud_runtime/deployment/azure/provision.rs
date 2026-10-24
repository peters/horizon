//! Provisions an Azure cloud's worker through `horizon_cloud::azure::provision`,
//! keeping its records beside the deployment: the VM fence in the deployment record
//! and the group fence in the cloud's `azure.json`. Then waits for the VM to run and
//! the worker to pass its contract over SSH: each pass reads the bound VM again with
//! the checks provisioning made, so a VM made again under the name, moved or resized
//! is refused, and the SSH probe is Hetzner's, with a scratch known-hosts file until
//! the contract is validated.
use super::{Compute, load, save};
use crate::cloud_runtime::{
    Error, Event, Result, Stage, WorkerContract,
    command::Runner,
    deployment::{
        Request,
        hetzner::readiness::{probe, remaining},
    },
    mutation::{self, State as Mutation},
    progress::Progress,
    ssh::Connection,
    state::{Deployment, Store},
    timeline::{AWAITING_ENDPOINT, AWAITING_SERVICES},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, WorkerSpec, WorkerStatus,
    azure::provision::{self as cloud, Journal, Records, inspect_worker},
    host::RegistryLogin,
};
use std::time::{Duration, Instant};

const PROBE_INTERVAL: Duration = Duration::from_secs(2);

pub(in crate::cloud_runtime::deployment) fn provision(
    compute: &Compute,
    store: &Store,
    state: &mut Deployment,
    spec: &WorkerSpec,
    cancel: &Cancellation,
    observe: mutation::Observer<'_>,
) -> Result<()> {
    let mut journal = load(store.root())?;
    if state.operation == CreateState::Requested || journal.group == CreateState::Requested {
        observe(Mutation::Pending)?;
    }
    // Resources are named after the deployment, and the custom data and the checks
    // follow the spec, so both must describe the same cloud before any request.
    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
        return Err(Error::Invalid("Deployment and worker identities differ"));
    }
    // Only a request that can still create the VM sends custom data, so only it
    // needs the pull login.
    let login = if state.operation == CreateState::Prepared {
        pull_login(compute, spec)?
    } else {
        None
    };
    let request = cloud::Request {
        spec,
        policy: &compute.allowed,
        login,
    };
    let mut operation = state.operation.clone();
    let mut saved = Saved {
        store,
        state: &mut *state,
        observe,
        failed: None,
    };
    let worker = cloud::provision(
        &compute.client,
        request,
        &mut operation,
        &mut journal,
        &mut saved,
        cancel,
    );
    let worker = saved.finish(worker)?;
    state.worker = Some(worker);
    store.save(state)?;
    observe(Mutation::Settled)?;
    Ok(())
}

pub(in crate::cloud_runtime::deployment) fn wait(
    request: &Request,
    compute: &Compute,
    store: &Store,
    runner: &Runner<'_>,
    state: &mut Deployment,
    spec: &WorkerSpec,
) -> Result<(Connection, WorkerContract)> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(state.profile.bootstrap.readiness_seconds));
    state.stage = Stage::Readiness;
    store.save(state)?;
    (runner.emit)(Event::stage(state.stage));
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("Missing worker identity"));
    };
    let region = load(store.root())?
        .region
        .ok_or(Error::Invalid("Missing Azure region of this cloud"))?;
    loop {
        remaining(deadline, runner)?;
        let described = inspect_worker(
            &compute.client,
            spec,
            &compute.allowed,
            &region,
            &worker_id,
            runner.cancel,
        )?;
        if state.worker.as_ref().map(|known| serde_json::to_value(known).ok())
            != Some(serde_json::to_value(&described).ok())
        {
            state.worker = Some(described.clone());
            store.save(state)?;
        }
        if described.status() == WorkerStatus::Running {
            (runner.emit)(Event::Progress(Progress::activity(AWAITING_SERVICES)));
            if let Some(ready) = probe(request, store, runner, &described, deadline)? {
                return Ok(ready);
            }
        } else {
            (runner.emit)(Event::Progress(Progress::activity(AWAITING_ENDPOINT)));
        }
        std::thread::sleep(PROBE_INTERVAL.min(remaining(deadline, runner)?));
    }
}

/// The login a verified registry grant gave, or the machine's Azure pull login. A
/// private image of a registry binding needs the grant verified for its digest.
fn pull_login(compute: &Compute, spec: &WorkerSpec) -> Result<Option<RegistryLogin>> {
    let bound = compute
        .registries
        .as_ref()
        .map(|config| config.select(&spec.profile.image))
        .transpose()?
        .flatten()
        .is_some();
    match compute.verified_pull.borrow_mut().take() {
        Some(verified) if verified.image == spec.image_digest => {
            if verified
                .expires_at
                .is_some_and(|expiry| expiry <= time::OffsetDateTime::now_utc().unix_timestamp())
            {
                return Err(Error::Invalid(
                    "Registry credential has expired; replace it before deploying",
                ));
            }
            Ok(Some(verified.login))
        }
        Some(_) => Err(Error::Invalid(
            "The verified pull grant belongs to another immutable image",
        )),
        None if bound => Err(Error::Invalid(
            "Verify this cloud's read-only image grant before allocation",
        )),
        None => compute.settings.registry_login(),
    }
}

/// The deployment record and `azure.json`, each saved durably. A save that fails is
/// kept, so the caller reports its cause rather than the provider layer's generic
/// persistence error.
struct Saved<'a> {
    store: &'a Store,
    state: &'a mut Deployment,
    observe: mutation::Observer<'a>,
    failed: Option<Error>,
}

impl Saved<'_> {
    fn finish<T>(self, result: std::result::Result<T, CloudError>) -> Result<T> {
        result.map_err(|error| match (error, self.failed) {
            (CloudError::Persistence, Some(failed)) => failed,
            (error, _) => error.into(),
        })
    }

    fn keep(&mut self, saved: Result<()>) -> std::result::Result<(), CloudError> {
        saved.map_err(|error| {
            self.failed = Some(error);
            CloudError::Persistence
        })
    }
}

impl Records for Saved<'_> {
    fn before_mutation(&mut self) -> std::result::Result<(), CloudError> {
        let observed = (self.observe)(Mutation::Pending).map(drop);
        self.keep(observed)
    }

    fn journal(&mut self, journal: &Journal) -> std::result::Result<(), CloudError> {
        let saved = save(journal, self.store.root());
        self.keep(saved)
    }

    fn operation(&mut self, operation: &CreateState) -> std::result::Result<(), CloudError> {
        self.state.operation = operation.clone();
        let saved = self.store.save(self.state);
        self.keep(saved)
    }
}
