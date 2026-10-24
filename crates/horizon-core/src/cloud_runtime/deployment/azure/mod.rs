//! Deployment on Azure: one VM runs the unchanged worker image under Docker and
//! keeps the workspace on a managed data disk, all inside the cloud's own resource
//! group. The VM is fenced by the deployment's `operation`; the group and its region
//! live in this cloud's `azure.json` journal. A stop deallocates the VM, Resume
//! starts it again, and a delete removes the group and proves it absent.
//!
//! `providers::preflight` admits an Azure cloud that the settings can make, and
//! `offers::AZURE_DEPLOYABLE` lets every caller create one. A worker is placed by
//! the settings' VM sizes and regions, never by a single chosen offer.
mod lifecycle;
mod provision;
#[cfg(test)]
mod tests;

pub(in crate::cloud_runtime) use lifecycle::{check, client, delete, resume, stop};
pub(super) use provision::{provision, wait};

use super::{Error, Result, Settings};
use horizon_cloud::azure::{Azure, provision::Journal, provision::Policy};
use std::{io::Write as _, path::Path};

const JOURNAL: &str = "azure.json";

/// The ARM client and the machine's Azure settings for one deployment.
pub(in crate::cloud_runtime) struct Compute {
    pub(super) client: Azure,
    pub(super) settings: crate::cloud_runtime::settings::Azure,
    /// The sizes and regions the current settings allow; every attempt, retry and
    /// reconnect is checked against these.
    pub(super) allowed: Policy,
    /// The machine's private registry bindings, to tell whether an image needs a pull login.
    pub(super) registries: Option<crate::cloud_runtime::registry::Config>,
    pub(super) verified_pull: std::cell::RefCell<Option<super::hetzner::VerifiedPull>>,
}

impl Compute {
    pub(in crate::cloud_runtime) fn new(settings: &Settings) -> Result<Self> {
        let azure = settings.azure_settings()?.clone();
        let client = Azure::new(azure.credential()?, &azure.subscription_id)?;
        Ok(Self::with_client(client, azure, settings.registries.clone()))
    }

    pub(super) fn with_client(
        client: Azure,
        settings: crate::cloud_runtime::settings::Azure,
        registries: Option<crate::cloud_runtime::registry::Config>,
    ) -> Self {
        Self {
            client,
            allowed: Policy {
                sizes: settings.vm_sizes.clone(),
                regions: settings.regions.clone(),
            },
            settings,
            registries,
            verified_pull: std::cell::RefCell::default(),
        }
    }
}

/// Whether the cloud's resource group, and so anything in it, may exist on Azure.
pub(in crate::cloud_runtime) fn retained(root: &Path) -> Result<bool> {
    Ok(load(root)?.retains())
}

/// Whether the cloud's deletion has started, so nothing may reconnect or create it.
pub(in crate::cloud_runtime) fn deleting(root: &Path) -> Result<bool> {
    Ok(load(root)?.deleting)
}

/// The journal in the cloud's own `azure.json`, beside its deployment record.
pub(super) fn load(root: &Path) -> Result<Journal> {
    match std::fs::read(root.join(JOURNAL)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(error.into()),
    }
}

/// Syncs the file and its directory, as the deployment record does.
pub(super) fn save(journal: &Journal, root: &Path) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(journal).map_err(|_| Error::Json)?;
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(root.join(JOURNAL)).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
    Ok(())
}
