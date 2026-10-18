//! The host file: the `browser.remote` providers and targets that the desktop
//! configuration uses, and, for each provider, the private files that hold its
//! credential values. The host file itself holds no credential value.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use horizon_browser::provider_catalog::{self, CatalogCache};
use horizon_browser::remote::{CredentialReference, RemoteBrowserConfig, RemoteProviderProfile};
use horizon_browser::{RemoteAuthorizationHeader, RemoteSessionRequest};
use horizon_browser_control::remote_authorization::{self, MAX_SECRET_BYTES};
use horizon_browser_control::remote_slots::quota_key;
use serde::Deserialize;
use zeroize::Zeroizing;

const MAX_HOST_FILE_BYTES: u64 = 256 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostFile {
    browser: BrowserSection,
    #[serde(default)]
    credential_files: BTreeMap<String, BTreeMap<CredentialReference, PathBuf>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserSection {
    remote: RemoteBrowserConfig,
}

/// A validated host file. It names credential files and never holds a value.
#[derive(Debug)]
pub struct HostConfig {
    pub remote: RemoteBrowserConfig,
    credential_files: BTreeMap<String, BTreeMap<CredentialReference, PathBuf>>,
}

impl HostConfig {
    /// Read and check the host file at `path`.
    ///
    /// # Errors
    /// The file is missing, too large, writable by other users, not valid
    /// YAML or JSON, or names an invalid provider, target or credential file.
    pub fn load(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() || metadata.len() > MAX_HOST_FILE_BYTES {
            return Err(io::Error::other(format!(
                "host file {} must be a regular file of at most {MAX_HOST_FILE_BYTES} bytes",
                path.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // The host file decides where credentials are sent.
            if metadata.permissions().mode() & 0o022 != 0 {
                return Err(io::Error::other(format!(
                    "host file {} must not be writable by group or other users",
                    path.display()
                )));
            }
        }
        let file: HostFile = serde_yaml::from_slice(&std::fs::read(path)?)
            .map_err(|error| io::Error::other(format!("host file {} is invalid: {error}", path.display())))?;
        Self::new(file.browser.remote, file.credential_files)
    }

    fn new(
        remote: RemoteBrowserConfig,
        credential_files: BTreeMap<String, BTreeMap<CredentialReference, PathBuf>>,
    ) -> io::Result<Self> {
        remote
            .validate_definition()
            .map_err(|error| io::Error::other(format!("remote configuration is invalid: {error}")))?;
        if remote.providers.is_empty() {
            return Err(io::Error::other("host file configures no remote provider"));
        }
        if let Some(name) = credential_files
            .keys()
            .find(|name| !remote.providers.contains_key(*name))
        {
            return Err(io::Error::other(format!(
                "credential_files names provider `{name}`, which is not configured"
            )));
        }
        for (name, profile) in &remote.providers {
            if !profile.credential_bindings.is_empty() {
                return Err(io::Error::other(format!(
                    "provider `{name}` has credential_bindings; this host reads credentials only from credential_files"
                )));
            }
            let required: BTreeSet<_> = profile.authentication.references().into_iter().collect();
            let files = credential_files.get(name);
            let named: BTreeSet<_> = files.into_iter().flat_map(BTreeMap::keys).collect();
            if let Some(reference) = required.symmetric_difference(&named).next() {
                return Err(io::Error::other(format!(
                    "provider `{name}` needs exactly one credential file for each authentication reference; check `{}`",
                    reference.as_str()
                )));
            }
            if let Some((reference, _)) = files.into_iter().flatten().find(|(_, path)| !path.is_absolute()) {
                return Err(io::Error::other(format!(
                    "credential file of `{name}`.`{}` must be an absolute path",
                    reference.as_str()
                )));
            }
        }
        Ok(Self {
            remote,
            credential_files,
        })
    }

    /// # Errors
    /// `provider_unknown` when no provider has this name.
    pub fn provider(&self, name: &str) -> io::Result<&RemoteProviderProfile> {
        self.remote
            .providers
            .get(name)
            .ok_or_else(|| io::Error::other(format!("provider_unknown: provider `{name}` is not configured")))
    }

    /// The authorization header of provider `name`, read from its files now,
    /// so a changed or removed file applies to the next session.
    ///
    /// # Errors
    /// A credential file is missing, readable by other users, or holds a value
    /// that does not fit the scheme. Messages name the reference, never the value.
    pub fn authorization(&self, name: &str) -> io::Result<Option<Zeroizing<String>>> {
        let profile = self.provider(name)?;
        let files = self.credential_files.get(name);
        remote_authorization::authorization_header(
            &profile.authentication,
            |reference| {
                let path = files
                    .and_then(|files| files.get(reference))
                    .ok_or_else(|| credential_error(name, reference, "has no credential file"))?;
                read_value(path).map_err(|problem| {
                    // The operator sees the path; the MCP client sees only the reference.
                    tracing::warn!(provider = name, reference = reference.as_str(), path = %path.display(), problem, "credential file is not ready");
                    credential_error(name, reference, &problem)
                })
            },
            |reference, error| credential_error(name, reference, &error.to_string()),
        )
    }

    /// The driver request for `target`: a configured target, or a provider
    /// device that `catalog` listed.
    ///
    /// # Errors
    /// The target or its provider is unknown, or the credentials are not ready.
    pub fn request(&self, target: &str, catalog: &CatalogCache) -> io::Result<RemoteSessionRequest> {
        let device = provider_catalog::target_provider(target)
            .map(|provider| {
                catalog
                    .target(self.provider(provider)?, target)
                    .map_err(|error| io::Error::other(format!("provider_catalog_refresh_required: {error}")))
            })
            .transpose()?;
        let synthesized = device.map(provider_catalog::target_profile);
        let profile = synthesized
            .as_ref()
            .or_else(|| self.remote.targets.get(target))
            .ok_or_else(|| io::Error::other(format!("target_unknown: remote target `{target}` is not configured")))?;
        let provider = self.provider(&profile.provider)?;
        let authorization = self
            .authorization(&profile.provider)?
            .map(|header| RemoteAuthorizationHeader::new(header.as_str().to_owned()).map(Arc::new))
            .transpose()
            .map_err(|_| io::Error::other("credentials_invalid: the authorization is not a header value"))?;
        let mut request = horizon_browser::remote_config::configured_remote_request(
            provider,
            profile,
            target,
            authorization,
            quota_key(provider),
        )
        .map_err(|error| io::Error::other(format!("target_invalid: {error}")))?;
        if let Some(device) = device {
            provider_catalog::apply_catalog_options(&mut request, device);
        }
        Ok(request)
    }
}

fn credential_error(provider: &str, reference: &CredentialReference, problem: &str) -> io::Error {
    io::Error::other(format!(
        "credentials_not_ready: provider `{provider}` credential `{}` {problem}",
        reference.as_str()
    ))
}

/// One credential value from a private file, without one final line break.
/// Problems name neither the value nor the path.
fn read_value(path: &Path) -> Result<Zeroizing<String>, String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("file cannot be opened ({})", error.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("file cannot be read ({})", error.kind()))?;
    if !metadata.is_file() {
        return Err("file is not a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("file must be readable only by its owner".into());
        }
    }
    // One allocation of the full bound, so no partial copy is left behind by growth.
    let limit = MAX_SECRET_BYTES + 2;
    let mut bytes = Zeroizing::new(Vec::with_capacity(limit + 1));
    (&mut file)
        .take(u64::try_from(limit + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("file cannot be read ({})", error.kind()))?;
    let value = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(&bytes);
    remote_authorization::header_safe_value(value).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(super) fn for_tests(
    remote: RemoteBrowserConfig,
    credential_files: BTreeMap<String, BTreeMap<CredentialReference, PathBuf>>,
) -> io::Result<HostConfig> {
    HostConfig::new(remote, credential_files)
}
