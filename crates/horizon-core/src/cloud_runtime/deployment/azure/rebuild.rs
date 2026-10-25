//! The Azure half of an image rebuild (`provider::Rebuild::NewServer`). ARM reports
//! the host, not the container, so a rebuild moves the cloud to a new VM. An Azure
//! stop deallocates and keeps the VM (`StoppedCost::WorkerKept`), so unlike Hetzner
//! the release is not a stop: `horizon_cloud::azure::release` deletes only the VM and
//! keeps the workspace disk, the static address and the network. The reconnect after
//! the rebuild deploys the same template again, which makes a new VM on that disk.
//!
//! The release is fenced in the cloud's `azure-release.json`, saved just before the
//! VM's delete request; `azure.json` is not changed. A VM the release refuses is never
//! recorded, so cancelling the rebuild then keeps it. The rebuild journal keeps the
//! deployment in the `Replace` stage throughout, so nothing here records a stop.
use super::{Error, Result, client, load as load_journal, persist};
use crate::cloud_runtime::{
    settings::Settings,
    state::{Deployment, Store},
};
use horizon_cloud::{
    Cancellation, CloudError, CreateState, azure::Azure, hetzner::cloud::released as without_endpoint,
    host::RegistryLogin,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

const RECORD: &str = "azure-release.json";

/// The release of the cloud's VM for a rebuild.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    /// The `vmId` whose release a rebuild began. Saved before the delete request and
    /// cleared with the VM fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) released: Option<String>,
    /// A VM held the workspace disk and was released, so a reconnect may make a new
    /// VM on that disk. Never cleared.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) held: bool,
}

pub(super) fn load(root: &Path) -> Result<Release> {
    match std::fs::read(root.join(RECORD)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Release::default()),
        Err(error) => Err(error.into()),
    }
}

fn save(release: &Release, root: &Path) -> Result<()> {
    persist(
        &serde_json::to_vec_pretty(release).map_err(|_| Error::Json)?,
        root,
        RECORD,
    )
}

/// Deletes the bound VM and keeps its workspace disk, address and network, but
/// leaves the deployment in its `Replace` stage. A retry after an interruption
/// finishes the release: the VM is read again and its delete is not sent twice.
/// # Errors
/// Refuses another subscription than the cloud's and a cloud that is being deleted,
/// and reports provider and persistence failures, leaving records a retry resumes from.
pub(in crate::cloud_runtime) fn release(
    store: &Store,
    state: &mut Deployment,
    settings: &Settings,
    cancel: &Cancellation,
) -> Result<()> {
    release_with(&client(settings, store)?, store, state, cancel)
}

pub(super) fn release_with(client: &Azure, store: &Store, state: &mut Deployment, cancel: &Cancellation) -> Result<()> {
    let CreateState::Bound { worker_id } = state.operation.clone() else {
        return Err(Error::Invalid("Only a bound VM can be released for a rebuild"));
    };
    if load_journal(store.root())?.deleting {
        return Err(Error::Invalid(
            "The cloud is being deleted; finish deleting it instead of rebuilding",
        ));
    }
    let fenced = Release {
        released: Some(worker_id.clone()),
        held: true,
    };
    let fence = || -> Result<()> {
        if load(store.root())? == fenced {
            return Ok(());
        }
        save(&fenced, store.root())
    };
    let failed = std::cell::RefCell::new(None);
    let mut announce = || {
        fence().map_err(|error| {
            failed.replace(Some(error));
            CloudError::Persistence
        })
    };
    let released = horizon_cloud::azure::release::release(client, &state.cloud_id, &worker_id, cancel, &mut announce);
    // A record that could not be saved is reported as itself, not as the persistence stand-in.
    if let Some(error) = failed.take() {
        return Err(error);
    }
    released?;
    // A VM proven gone was released, even when an earlier attempt sent its delete.
    fence()?;
    // The last known worker, as a released worker with no endpoint.
    state.worker = state.worker.as_ref().map(without_endpoint).transpose()?;
    store.save(state)
}

/// Saves `state`, the rebuild's outcome, with the fence of the released VM cleared,
/// then clears the release. The next reconnect deploys a new VM on the same disk.
/// # Errors
/// Refuses unless the bound VM is the one released.
pub(in crate::cloud_runtime) fn reopen(store: &Store, state: &mut Deployment) -> Result<()> {
    let mut release = load(store.root())?;
    let CreateState::Bound { worker_id } = &state.operation else {
        return Err(Error::Invalid("Only a bound VM can be released for a rebuild"));
    };
    if release.released.as_deref() != Some(worker_id.as_str()) {
        return Err(Error::Invalid(
            "The VM has not been released yet; continue the rebuild to release it",
        ));
    }
    state.operation = CreateState::Prepared;
    state.worker = None;
    state.stop_requested = false;
    store.save(state)?;
    // Saved second: a crash in between leaves a release that no bound VM matches,
    // and `held` already lets the reconnect make the new VM.
    release.released = None;
    save(&release, store.root())
}

/// Whether a rebuild's release of the bound VM has begun: it is recorded before the
/// delete request, so without it the VM is untouched.
/// # Errors
/// Reports an unreadable record.
pub(in crate::cloud_runtime) fn released(store: &Store, state: &Deployment) -> Result<bool> {
    let release = load(store.root())?;
    Ok(
        matches!(&state.operation, CreateState::Bound { worker_id } if release.released.as_deref() == Some(worker_id.as_str())),
    )
}

/// The pull login the host of the next VM uses for `image`, checked before the VM is
/// released: the machine's registry binding for the image, else the `azure` settings'
/// `registry_pull`, as provisioning chooses it.
/// # Errors
/// Refuses missing `azure` settings, an unreadable login and a login for another
/// registry than the image's.
pub(in crate::cloud_runtime) fn pull_login(settings: &Settings, image: &str) -> Result<Option<RegistryLogin>> {
    let azure = settings.azure_settings()?;
    let binding = settings
        .registries
        .as_ref()
        .map(|config| config.select(image))
        .transpose()?
        .flatten();
    let login = match binding {
        Some(binding) => Some(binding.host_login()?),
        None => azure.registry_login()?,
    };
    horizon_cloud::azure::provision::check_login(login.as_ref(), image)?;
    Ok(login)
}

/// Whether a VM of this cloud held its workspace disk and was released for a rebuild,
/// so a reconnect without a bound VM makes the next one rather than a first one.
/// # Errors
/// Reports an unreadable record.
pub(in crate::cloud_runtime) fn held(root: &Path) -> Result<bool> {
    Ok(load(root)?.held)
}
