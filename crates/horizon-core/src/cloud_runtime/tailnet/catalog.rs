//! Saved catalog removal retains cloud ownership through credential deletion.
use super::{Catalog, Error, Result, Selection, mapped, state, store};
use horizon_cloud::tailnet::CatalogOwnership;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Enumerated,
    Owned,
}

/// Remove a saved network only when no allocation or pending request needs it.
/// # Errors
/// Busy cloud ownership, changed directory membership, corrupt state or retained resources.
pub fn remove_saved(root: &Path, id: &str) -> Result<Catalog> {
    remove_with(root, id, |_| Ok(()), |ownership| ownership.delete(id).map_err(mapped))
}

fn remove_with(
    root: &Path,
    id: &str,
    mut checkpoint: impl FnMut(Boundary) -> Result<()>,
    commit: impl FnOnce(&CatalogOwnership) -> Result<Catalog>,
) -> Result<Catalog> {
    if !horizon_cloud::tailnet::valid_id(id) {
        return Err(Error::Invalid("Invalid saved tailnet identity"));
    }
    let paths = cloud_paths(root)?;
    checkpoint(Boundary::Enumerated)?;
    // Every writer takes its cloud lock before the existing catalog mutation lock.
    // Keep this complete, sorted set until catalog and credential deletion settle.
    let clouds = paths
        .clouds
        .iter()
        .map(|path| state::Store::lock(path))
        .collect::<Result<Vec<_>>>()?;
    let ownership = store(root).own_catalog().map_err(mapped)?;
    if cloud_paths(root)? != paths {
        return Err(Error::Busy);
    }
    let catalog = ownership.load().map_err(mapped)?;
    if !catalog.tailnets.iter().any(|network| network.id == id) {
        return Err(Error::Invalid("The saved tailnet no longer exists"));
    }
    checkpoint(Boundary::Owned)?;
    for cloud in &clouds {
        let selected = Selection::load(cloud.root()).map_err(mapped)?;
        let requested = super::super::companions::lifecycle::requested_tailnet(cloud.root())?;
        let deployment = cloud.load()?;
        if requested.is_some_and(|selection| selection.tailnet.as_deref() == Some(id)) {
            return Err(Error::Invalid(
                "Cancel the pending cloud request before removing its tailnet",
            ));
        }
        if selected.tailnet.as_deref() != Some(id) {
            continue;
        }
        if let Some(deployment) = deployment {
            if (deployment.stage != super::super::Stage::Deleted
                && (deployment.spec.is_some()
                    || deployment.worker.is_some()
                    || deployment.operation != super::super::CreateState::Prepared))
                || !super::super::lifecycle::can_remove(cloud, &deployment)?
            {
                return Err(Error::Invalid(
                    "Delete clouds assigned to this tailnet before removing its credentials",
                ));
            }
        } else if provider_record_exists(cloud.root())? {
            return Err(Error::Invalid(
                "Provider state exists without a deployment; preserve the tailnet",
            ));
        }
    }
    let current = cloud_paths(root)?;
    if current.settings != paths.settings || current.bindings != paths.bindings {
        return Err(Error::Busy);
    }
    commit(&ownership)
}

#[derive(PartialEq, Eq)]
struct DirectorySet {
    clouds: BTreeSet<PathBuf>,
    settings: BTreeMap<PathBuf, BTreeSet<std::ffi::OsString>>,
    bindings: Option<Vec<u8>>,
}

fn cloud_paths(root: &Path) -> Result<DirectorySet> {
    let registry = registry_directory(root)?;
    let mut clouds = BTreeSet::new();
    let mut settings = BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if !kind.is_dir() && !kind.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        // Legacy allocation journals are not additional cloud identities.
        if name == ".allocations" && kind.is_dir() {
            continue;
        }
        if kind.is_symlink() || !name.to_str().is_some_and(horizon_cloud::valid_id) {
            return Err(Error::Invalid("Cloud catalog contains an unknown directory"));
        }
        if let Some(files) = settings_directory(&entry.path(), &name, &registry)? {
            settings.insert(entry.path(), files);
            continue;
        }
        clouds.insert(entry.path());
    }
    Ok(DirectorySet {
        clouds,
        settings,
        bindings: registry.bytes,
    })
}

// Setup secrets and SSH keys share the settings root with cloud journals. Recognize
// their exact file namespace without reading key material; mixed state stays fenced.
fn settings_directory(
    path: &Path,
    name: &std::ffi::OsStr,
    bindings: &RegistrySettings,
) -> Result<Option<BTreeSet<std::ffi::OsString>>> {
    let credentials = name == "credentials";
    let identity = name.to_str().is_some_and(|name| name.starts_with("identity-"));
    let registry_files = bindings
        .directory
        .as_ref()
        .filter(|registry| registry.path == path)
        .map(|registry| &registry.files);
    if !credentials && !identity && registry_files.is_none() {
        return Ok(None);
    }
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let valid = name.to_str().is_some_and(|name| {
            if credentials {
                !cloud_metadata_file(name)
                    && (credential_file(name) || bindings.credentials.contains(std::ffi::OsStr::new(name)))
            } else if identity {
                matches!(name, "ed25519" | "ed25519.pub")
            } else {
                registry_files.is_some_and(|files| files.contains(std::ffi::OsStr::new(name)))
                    && !cloud_metadata_file(name)
            }
        });
        if !entry.file_type()?.is_file() || !valid {
            // A valid cloud ID may collide with a setup namespace. Never ignore
            // its selection, provider state or unfinished operation evidence.
            return Err(Error::Invalid("Cloud state collides with a settings directory"));
        }
        files.insert(name);
    }
    if identity && files.len() != 2 {
        return Err(Error::Invalid("Incomplete settings SSH identity directory"));
    }
    Ok(Some(files))
}

// Settings contain credential paths, not credential values. Do not call the loader:
// its GitHub status side effect is unrelated to a catalog ownership check.
struct RegistryDirectory {
    path: PathBuf,
    files: BTreeSet<std::ffi::OsString>,
}

struct RegistrySettings {
    credentials: BTreeSet<std::ffi::OsString>,
    bytes: Option<Vec<u8>>,
    directory: Option<RegistryDirectory>,
}

fn registry_directory(root: &Path) -> Result<RegistrySettings> {
    let bytes = match std::fs::read(root.join("settings.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RegistrySettings {
                credentials: BTreeSet::new(),
                bytes: None,
                directory: None,
            });
        }
        Err(error) => return Err(error.into()),
    };
    let settings: super::super::settings::Settings = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    settings.validate()?;
    let credentials = configured_credentials(root, &settings);
    let registry = settings
        .registries
        .filter(|config| config.root.parent() == Some(root))
        .map(|config| {
            let files = config
                .bindings
                .iter()
                .flat_map(|binding| std::iter::once(&binding.generation).chain(&binding.retired))
                .flat_map(|generation| [format!("{generation}.json").into(), format!("{generation}.lock").into()])
                .collect();
            RegistryDirectory {
                path: config.root,
                files,
            }
        });
    Ok(RegistrySettings {
        credentials,
        bytes: Some(bytes),
        directory: registry,
    })
}

fn configured_credentials(root: &Path, settings: &super::super::settings::Settings) -> BTreeSet<std::ffi::OsString> {
    let providers = std::iter::once(&settings.runpod_key_file)
        .chain(settings.hetzner.iter().map(|provider| &provider.token_file))
        .chain(
            settings
                .hetzner
                .iter()
                .filter_map(|provider| provider.registry_pull.as_ref())
                .map(|pull| &pull.password_file),
        );
    let agents = settings
        .openai_api_key_file
        .iter()
        .chain(settings.anthropic_api_key_file.iter());
    let registries = settings
        .registries
        .iter()
        .flat_map(|config| &config.bindings)
        .flat_map(|binding| {
            std::iter::once(&binding.pull.secret_file).chain(binding.publish.iter().map(|auth| &auth.secret_file))
        });
    let grants = settings
        .git_credentials
        .iter()
        .map(|binding| &binding.token_file)
        .chain(
            settings
                .browserstack_credentials
                .iter()
                .map(|binding| &binding.configuration_file),
        )
        .chain(settings.github.iter().map(|app| &app.client_secret_file))
        .chain([&settings.ssh_identity_file, &settings.docker_config]);
    let directory = root.join("credentials");
    providers
        .chain(agents)
        .chain(registries)
        .chain(grants)
        .filter(|path| path.parent() == Some(directory.as_path()))
        .filter_map(|path| path.file_name().map(std::ffi::OsStr::to_owned))
        .collect()
}

fn cloud_metadata_file(name: &str) -> bool {
    matches!(
        name,
        "deployment.json"
            | "tailnet.json"
            | "hetzner.json"
            | "workspace-volume.json"
            | "workspace-volume.required"
            | "companion-operation.json"
            | "operation.lock"
            | "companion-execution.lock"
            | "companions.json"
            | "allocation.json"
            | "project.json"
    ) || name.starts_with("tailnet-request-")
        || name.starts_with("tailnet-commit-")
}

fn credential_file(name: &str) -> bool {
    [
        "compute",
        "hetzner",
        "openai",
        "anthropic",
        "registry-pull",
        "registry-push",
        "github-app",
    ]
    .iter()
    .any(|prefix| {
        name == *prefix
            || name.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.starts_with('-')
                    && suffix.len() > 1
                    && suffix[1..]
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
    })
}

fn provider_record_exists(cloud: &Path) -> Result<bool> {
    for name in ["hetzner.json", "workspace-volume.json", "workspace-volume.required"] {
        match std::fs::symlink_metadata(cloud.join(name)) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

/// Caller retains cloud ownership through any later provider allocation.
pub(super) fn validate_selection(cloud: &Path) -> Result<()> {
    if Selection::load(cloud).map_err(mapped)?.tailnet.is_none() {
        return Ok(());
    }
    let root = cloud.parent().ok_or(Error::Invalid("Missing cloud catalog root"))?;
    let ownership = store(root).own_catalog().map_err(mapped)?;
    let catalog = ownership.load().map_err(mapped)?;
    let selection = Selection::load(cloud).map_err(mapped)?;
    if selection
        .tailnet
        .as_deref()
        .is_some_and(|id| !catalog.tailnets.iter().any(|network| network.id == id))
    {
        return Err(Error::Invalid(
            "The selected tailnet is no longer saved; refuse allocation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
