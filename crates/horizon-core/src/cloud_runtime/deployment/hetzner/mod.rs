//! Deployment on Hetzner Cloud: a server runs the unchanged worker image under
//! Docker and keeps the workspace on a volume in one location. The server is
//! fenced by the deployment's `operation`; the volume, the chosen location and
//! the SSH key live in this cloud's `hetzner.json` journal, so `RunPod` clouds and
//! their records are untouched.
pub(in crate::cloud_runtime) mod lifecycle;
mod provision;
mod readiness;
#[cfg(test)]
mod tests;

pub(super) use provision::provision;

/// Checks what a Hetzner cloud needs before any record, build or provider
/// request, so an unsupported request fails at once and leaves nothing behind.
pub(super) fn preflight(cloud_id: &str, profile: &horizon_cloud::Profile, settings: &Settings) -> Result<()> {
    if !horizon_cloud::hetzner::volumes::SIZE_GB.contains(&u32::from(profile.storage.volume_gb)) {
        return Err(Error::Invalid(
            "A Hetzner workspace volume must be between 10 and 10,240 GB",
        ));
    }
    horizon_cloud::hetzner::resource_name(cloud_id).map_err(|_| {
        Error::Invalid("A Hetzner cloud needs an ID of lowercase letters, digits and hyphens, at most 48 characters")
    })?;
    if profile.idle_stop_minutes.is_some() {
        return Err(Error::Invalid(
            "idle_stop_minutes is not available on Hetzner yet; a Hetzner worker cannot stop its own billing",
        ));
    }
    let hetzner = super::sizing::hetzner(settings)?;
    hetzner.credential()?;
    hetzner.locations_for(settings.placement.as_ref())?;
    Ok(())
}

/// Checks the pull login before the deployment is recorded or its image built,
/// unless the server is already requested or bound and so only reconciled.
pub(super) fn admit(settings: &Settings, image: &str, operation: &CreateState) -> Result<()> {
    if matches!(operation, CreateState::Requested | CreateState::Bound { .. }) {
        return Ok(());
    }
    pull_login(super::sizing::hetzner(settings)?, settings.registries.as_ref(), image).map(drop)
}

/// The pull login a host needs for `image`, which provisioning loads only while
/// a server can still be created.
/// # Errors
/// Refuses a private image without a login, a login for another registry and
/// a spelling Docker would look up under a key the host does not write.
pub(super) fn pull_login(
    hetzner: &crate::cloud_runtime::settings::Hetzner,
    registries: Option<&crate::cloud_runtime::registry::Config>,
    image: &str,
) -> Result<Option<horizon_cloud::host::RegistryLogin>> {
    // A private image the machine has a registry binding for needs a pull login
    // on the host; without one the host's pull would fail after allocation.
    let private = registries
        .map(|config| config.select(image))
        .transpose()?
        .flatten()
        .is_some();
    let login = hetzner.registry_login()?;
    horizon_cloud::hetzner::cloud::check_login(login.as_ref(), private, image)?;
    Ok(login)
}

/// Whether anything this cloud created on Hetzner may still exist: the
/// workspace volume, or the SSH key, which is recorded before it is registered.
pub(in crate::cloud_runtime) fn retained(root: &Path) -> Result<bool> {
    Ok(Journal::load(root)?.retains())
}
pub(super) use readiness::wait;

use super::{Error, Result, Settings};
pub(super) use horizon_cloud::hetzner::cloud::{Journal, Policy as Allowed, throwaway_public_key, worker};
use horizon_cloud::{CreateState, hetzner::Hetzner};
use std::{io::Write as _, path::Path};

const JOURNAL: &str = "hetzner.json";

/// The Hetzner client and the machine's Hetzner settings for one deployment.
pub(super) struct Compute {
    pub(super) client: Hetzner,
    pub(super) settings: crate::cloud_runtime::settings::Hetzner,
    /// Where and on what this cloud may run under the current settings and its
    /// placement. Every attempt, retry and reconnect is checked against these,
    /// not against the policy recorded when the cloud was first provisioned.
    pub(super) allowed: Allowed,
    /// The machine's private registry bindings, to tell whether an image needs a pull login.
    pub(super) registries: Option<crate::cloud_runtime::registry::Config>,
}

impl Compute {
    pub(super) fn new(settings: &Settings) -> Result<Self> {
        let hetzner = super::sizing::hetzner(settings)?.clone();
        let allowed = Allowed {
            locations: hetzner.locations_for(settings.placement.as_ref())?,
            server_types: hetzner.server_types.clone(),
        };
        Ok(Self {
            client: Hetzner::new(hetzner.credential()?),
            settings: hetzner,
            allowed,
            registries: settings.registries.clone(),
        })
    }

    /// For stopping and deleting, which place nothing: resources are checked by
    /// ownership alone, so a changed placement policy never blocks their cleanup.
    pub(super) fn cleanup(settings: &Settings) -> Result<Self> {
        let hetzner = super::sizing::hetzner(settings)?.clone();
        Ok(Self {
            client: Hetzner::new(hetzner.credential()?),
            settings: hetzner,
            allowed: Allowed {
                locations: Vec::new(),
                server_types: Vec::new(),
            },
            registries: None,
        })
    }
}

/// The journal lives in the cloud's own `hetzner.json`, beside its deployment record.
pub(super) trait JournalFile: Sized {
    fn load(root: &Path) -> Result<Self>;
    fn save(&self, root: &Path) -> Result<()>;
}

impl JournalFile for Journal {
    fn load(root: &Path) -> Result<Self> {
        match std::fs::read(root.join(JOURNAL)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Json),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Syncs the file and its directory, as the deployment record does.
    fn save(&self, root: &Path) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| Error::Json)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(root.join(JOURNAL)).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }
}
