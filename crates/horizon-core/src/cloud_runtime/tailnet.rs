//! Thin deployment adapter for cloud-only tailnet enrollment.
mod keychain;
use super::{Error, Result, command::Runner, ssh::Connection, state};
pub use horizon_cloud::tailnet::{Catalog, Selection, Store, Tailnet, valid_key};
use std::{path::Path, time::Duration};

#[must_use]
pub fn store(root: &Path) -> Store {
    Store::new(root.to_path_buf())
}
fn mapped(_: horizon_cloud::tailnet::Error) -> Error {
    Error::Invalid("Tailnet settings or OS credential are unavailable; open Settings > Tailnets")
}
#[derive(serde::Serialize)]
struct Enrollment<'a> {
    tailnet: Option<&'a str>,
    key: Option<&'a str>,
}
/// Called under the deployment lock, before any agent sessions are started.
/// # Errors
/// Refuses unavailable workers, unsafe permissions and inaccessible credentials.
pub fn configure(connection: &Connection, cloud: &Path, runner: &Runner<'_>) -> Result<()> {
    if !cloud.join("tailnet.json").exists() {
        return Ok(());
    }
    let selected = Selection::load(cloud).map_err(mapped)?;
    if selected.tailnet.is_none() {
        return Ok(());
    }
    let payload = |key: Option<&str>| {
        serde_json::to_string(&Enrollment {
            tailnet: selected.tailnet.as_deref(),
            key,
        })
        .map(zeroize::Zeroizing::new)
        .map_err(|_| Error::Json)
    };
    let invoke = |input: &str| {
        runner.private_exchange(
            &mut connection.pinned_command("horizon-worker-tailnet configure"),
            input.as_bytes(),
            Duration::from_secs(60),
        )
    };
    let reply = invoke(&payload(None)?)?;
    match reply.as_slice() {
        b"ready\n" => Ok(()),
        b"needs_key\n" => {
            let id = selected
                .tailnet
                .as_deref()
                .ok_or(Error::Invalid("Missing tailnet selection"))?;
            let root = cloud
                .parent()
                .ok_or(Error::Invalid("Missing cloud settings directory"))?;
            if !store(root).load().map_err(mapped)?.tailnets.iter().any(|t| t.id == id) {
                return Err(mapped(horizon_cloud::tailnet::Error::Missing));
            }
            let key = keychain::read(id).map_err(mapped)?;
            let key = std::str::from_utf8(&key)
                .ok()
                .filter(|key| valid_key(key))
                .ok_or_else(|| mapped(horizon_cloud::tailnet::Error::Invalid))?;
            if invoke(&payload(Some(key))?)? == b"ready\n" {
                Ok(())
            } else {
                Err(Error::Invalid("Tailnet enrollment was not confirmed"))
            }
        }
        _ => Err(Error::Invalid("Unexpected tailnet enrollment response")),
    }
}
/// # Errors
/// Caller-visible settings errors; every write uses the existing cloud lifecycle lock.
pub fn select(root: &Path, cloud_id: &str, id: Option<&str>) -> Result<()> {
    let cloud = state::cloud_directory(root, cloud_id)?;
    let _lock = state::Store::lock(&cloud)?;
    let catalog = store(root).load().map_err(mapped)?;
    Selection::save(&cloud, id, &catalog).map_err(mapped)
}

/// Choose a network before provisioning; a provisioned cloud retains its choice.
/// # Errors
/// Rejects changing the network after an allocation was requested, including uncertain
/// requests. Choose another network on a new cloud instead.
pub fn change(root: &Path, cloud_id: &str, id: Option<&str>) -> Result<()> {
    let cloud = state::cloud_directory(root, cloud_id)?;
    let lock = state::Store::lock(&cloud)?;
    let prior = Selection::load(&cloud).map_err(mapped)?;
    if lock
        .load()?
        .is_some_and(|s| s.spec.is_some() || s.worker.is_some() || s.operation != super::CreateState::Prepared)
    {
        if prior.tailnet.as_deref() == id {
            return Ok(());
        }
        return Err(Error::Invalid(
            "Tailnet is chosen only when provisioning a cloud; create a new cloud to choose another network",
        ));
    }
    let catalog = store(root).load().map_err(mapped)?;
    Selection::save(&cloud, id, &catalog).map_err(mapped)
}
