//! Selected remote targets use the same provider adaptation as Horizon's other hosts.
use horizon_browser::{RemoteAuthorizationHeader, RemoteSessionRequest, remote::RemoteBrowserConfig};
use serde::Deserialize;
use std::{collections::BTreeMap, io, path::Path, sync::Arc};
mod recovery;
const CONFIG: &str = "/run/horizon-credentials/browserstack.json";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Configuration {
    version: u32,
    pub(super) remote: RemoteBrowserConfig,
    pub(super) authorization: BTreeMap<String, horizon_browser::SecretString>,
    pub(super) quota_keys: BTreeMap<String, String>,
    pub(super) local_identifier: String,
    pub(super) local_ports: std::collections::BTreeSet<u16>,
}
pub(super) fn configuration_for(capabilities: &horizon_cloud::Capabilities) -> io::Result<Configuration> {
    let selected = capabilities
        .browserstack
        .as_ref()
        .ok_or_else(|| io::Error::other("Remote target is disabled by this cloud profile"))?;
    let config = configuration(Path::new(CONFIG))?;
    if config.version != 1
        || config.remote.providers.len() != 1
        || !config.remote.providers.contains_key(&selected.provider)
        || config.local_ports != selected.local_ports
        || !config
            .local_identifier
            .strip_prefix("horizon-")
            .is_some_and(horizon_cloud::valid_id)
    {
        return Err(io::Error::other(
            "Remote runtime configuration differs from this cloud profile",
        ));
    }
    Ok(config)
}

pub fn request(
    capabilities: &horizon_cloud::Capabilities,
    name: &str,
    catalog: &horizon_browser::provider_catalog::CatalogCache,
) -> io::Result<RemoteSessionRequest> {
    use horizon_browser::provider_catalog::{self, target_provider};
    let mut config = configuration_for(capabilities)?;
    let device = target_provider(name)
        .map(|provider| {
            let profile = config
                .remote
                .providers
                .get(provider)
                .ok_or_else(|| io::Error::other("Remote provider is not granted to this cloud"))?;
            catalog.target(profile, name).map_err(io::Error::other)
        })
        .transpose()?;
    let synthesized = device.map(provider_catalog::target_profile);
    let target = synthesized
        .as_ref()
        .or_else(|| config.remote.targets.get(name))
        .ok_or_else(|| io::Error::other("Remote target is not configured; discover provider devices first"))?;
    let provider = config
        .remote
        .providers
        .get(&target.provider)
        .filter(|p| p.adapter == horizon_browser::remote::RemoteAdapterKind::Browserstack)
        .ok_or_else(|| io::Error::other("Remote target provider is not supported"))?;
    let header = config
        .authorization
        .remove(&target.provider)
        .ok_or_else(|| io::Error::other("Remote credentials are unavailable"))?;
    let authorization = RemoteAuthorizationHeader::new(header.as_str().to_owned())
        .map_err(|_| io::Error::other("Invalid remote credentials"))?;
    let quota = config
        .quota_keys
        .get(&target.provider)
        .cloned()
        .ok_or_else(|| io::Error::other("Missing remote account identity"))?;
    let mut request = horizon_browser::remote_config::configured_remote_request(
        provider,
        target,
        name,
        Some(Arc::new(authorization)),
        quota,
    )
    .map_err(|_| io::Error::other("Invalid remote target options"))?;
    if let Some(device) = device {
        provider_catalog::apply_catalog_options(&mut request, device);
    }
    request.capabilities["bstack:options"]["local"] = (!config.local_ports.is_empty()).into();
    if !config.local_ports.is_empty() {
        request.capabilities["bstack:options"]["localIdentifier"] = config.local_identifier.into();
    }
    Ok(request)
}

fn configuration(path: &Path) -> io::Result<Configuration> {
    let meta = path.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other("Remote credentials must be private"));
        }
    }
    if !meta.is_file() || meta.len() > 256 * 1024 {
        return Err(io::Error::other("Invalid remote credential file"));
    }
    let config: Configuration =
        serde_json::from_slice(&std::fs::read(path)?).map_err(|_| io::Error::other("Invalid remote configuration"))?;
    config
        .remote
        .validate_definition()
        .map_err(|_| io::Error::other("Invalid remote target configuration"))?;
    Ok(config)
}

struct Held {
    id: String,
    owner: String,
    provider: String,
    allocation: horizon_browser::RemoteAllocation,
    restored: bool,
}
pub struct Allocations {
    held: BTreeMap<String, Held>,
    root: std::path::PathBuf,
    orphans: BTreeMap<String, Journal>,
}
#[derive(serde::Serialize, Deserialize)]
struct Journal {
    provider: String,
    reference: String,
    #[serde(default)]
    owner: Option<String>,
}

impl Allocations {
    pub fn new(root: std::path::PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        let mut orphans = BTreeMap::new();
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let journal = serde_json::from_slice(&std::fs::read(entry.path())?)
                .map_err(|_| io::Error::other("Invalid retained remote allocation record"))?;
            orphans.insert(entry.file_name().to_string_lossy().into_owned(), journal);
        }
        Ok(Self {
            held: BTreeMap::new(),
            root,
            orphans,
        })
    }
    pub fn ensure_recoverable(&self) -> io::Result<()> {
        if !self.orphans.is_empty()
            || self
                .held
                .values()
                .any(|held| held.restored && !held.allocation.is_released())
        {
            return Err(io::Error::other(
                "Worker service was lost with remote device allocations; verify their release at the provider before deleting this worker or starting more devices",
            ));
        }
        Ok(())
    }
    pub fn insert(&mut self, id: &str, actor: &str, request: &RemoteSessionRequest) -> io::Result<()> {
        self.ensure_recoverable()?;
        let journal = Journal {
            provider: request.provider.clone(),
            reference: request.recovery.reference().into(),
            owner: Some(actor.into()),
        };
        let path = self.root.join(id);
        let identities = self.root.join("identities");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        let retained = (|| -> io::Result<()> {
            serde_json::to_writer(&mut file, &journal).map_err(io::Error::other)?;
            file.sync_all()?;
            std::fs::create_dir_all(&identities)?;
            #[cfg(unix)]
            std::fs::File::open(&self.root)?.sync_all()?;
            request.recovery.retain_journal(&identities.join(id), request)
        })();
        drop(file);
        if let Err(error) = retained {
            // Nothing was handed to a driver, so this admission cannot own compute.
            self.confirm_closed(id)?;
            return Err(error);
        }
        request
            .recovery
            .record_admission(horizon_browser_control::manifest::host_instance(), actor, "cloud");
        self.held.insert(
            request.recovery.reference().into(),
            Held {
                id: id.into(),
                owner: actor.into(),
                provider: request.provider.clone(),
                allocation: request.recovery.clone(),
                restored: false,
            },
        );
        while self.held.len() > 128 {
            let old = self
                .held
                .iter()
                .find(|(_, h)| h.allocation.is_released())
                .map(|(key, _)| key.clone());
            let Some(old) = old else { break };
            if let Some(held) = self.held.get(&old) {
                self.confirm_closed(&held.id)?;
            }
            self.held.remove(&old);
        }
        Ok(())
    }
    pub fn confirm_closed(&self, id: &str) -> io::Result<()> {
        if self.orphans.contains_key(id) {
            return Err(io::Error::other(
                "Remote device release is unconfirmed after worker service loss",
            ));
        }
        if self
            .held
            .values()
            .any(|held| held.id == id && !held.allocation.is_released())
        {
            return Err(io::Error::other("Remote device release is unconfirmed"));
        }
        let path = self.root.join(id);
        if path.exists() {
            std::fs::remove_file(path)?;
            #[cfg(unix)]
            std::fs::File::open(&self.root)?.sync_all()?;
        }
        let identity = self.root.join("identities").join(id);
        if identity.exists() {
            std::fs::remove_file(identity)?;
            #[cfg(unix)]
            std::fs::File::open(self.root.join("identities"))?.sync_all()?;
        }
        Ok(())
    }
    pub fn ids(&self) -> Vec<String> {
        self.held.values().map(|h| h.id.clone()).collect()
    }
    pub fn cancel_start(&mut self, id: &str) {
        for held in self.held.values().filter(|held| held.id == id) {
            held.allocation.cancel_before_launch();
        }
        let _ = self.confirm_closed(id);
    }
}

static USAGE_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
struct UsagePermit;
impl Drop for UsagePermit {
    fn drop(&mut self) {
        USAGE_WORKERS.fetch_sub(1, std::sync::atomic::Ordering::Release);
    }
}

pub fn poll(
    allocations: &mut Allocations,
    capabilities: &horizon_cloud::Capabilities,
    catalog: &mut super::catalog::Host,
) {
    use horizon_browser_control::manifest::{self, recovery};
    if let Ok(requests) = recovery::claim_recovery_requests(manifest::host_instance()) {
        if !requests.is_empty() {
            allocations.restore_retained(capabilities);
        }
        for request in requests {
            let mut error = None;
            if let Some(reference) = &request.reference {
                let authorized = allocations.held.get(reference).is_some_and(|held| {
                    held.allocation.reconcile_for(
                        manifest::host_instance(),
                        &request.actor,
                        "cloud",
                        held.owner == request.actor,
                    )
                });
                if !authorized {
                    error = Some("Allocation is not available to this agent".into());
                }
            }
            let mut summaries: Vec<_> = allocations
                .held
                .iter()
                .filter_map(|(reference, held)| {
                    held.allocation
                        .status_for(
                            manifest::host_instance(),
                            &request.actor,
                            "cloud",
                            held.owner == request.actor,
                        )
                        .map(|status| recovery::RemoteAllocationSummary {
                            reference: reference.clone(),
                            provider: held.provider.clone(),
                            status,
                            message: status.message().into(),
                        })
                })
                .collect();
            summaries.extend(allocations.orphans.values().map(|journal| recovery::RemoteAllocationSummary {
                reference: journal.reference.clone(), provider: journal.provider.clone(), status: horizon_browser::RemoteRecoveryStatus::IdentityUnavailable,
                message: "Worker browser service was lost. Verify the hosted session release at the provider; automatic allocation retry is blocked.".into(),
            }));
            let _ = recovery::complete_recovery(&request.result(summaries, error));
        }
    }
    let cleaned: Vec<_> = allocations
        .held
        .iter()
        .filter(|(_, held)| held.restored && held.allocation.is_released())
        .filter(|(_, held)| allocations.confirm_closed(&held.id).is_ok())
        .map(|(reference, _)| reference.clone())
        .collect();
    for reference in cleaned {
        if let Some(held) = allocations.held.get_mut(&reference) {
            held.restored = false;
        }
    }
    if let Ok(requests) = manifest::provider_usage::claim_provider_usage_requests(manifest::host_instance()) {
        for request in requests {
            // Catalog and cloud offer requests are answered, and retried, with the catalog.
            if request.catalog.is_some() || request.cloud_offers.is_some() {
                catalog.pending.push(request);
                continue;
            }
            if USAGE_WORKERS
                .fetch_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |n| (n < 4).then_some(n + 1),
                )
                .is_err()
            {
                let _ = manifest::provider_usage::complete_provider_usage(
                    &request.result(Vec::new(), Some("Remote usage is busy; retry later".into())),
                );
                continue;
            }
            let permit = UsagePermit;
            let enabled = capabilities.browserstack.is_some();
            std::thread::spawn(move || {
                let _permit = permit;
                let result = if enabled {
                    usage(request.provider.as_deref())
                } else {
                    Err(io::Error::other("Remote browsers are disabled by this cloud profile"))
                };
                let (providers, error) = match result {
                    Ok(providers) => (providers, None),
                    Err(error) => (Vec::new(), Some(error.to_string())),
                };
                let _ = manifest::provider_usage::complete_provider_usage(&request.result(providers, error));
            });
        }
    }
}

fn usage(
    selected: Option<&str>,
) -> io::Result<Vec<horizon_browser_control::manifest::provider_usage::ProviderUsageSummary>> {
    use horizon_browser::{
        provider_usage::{UsageAdapter, fetch_usage},
        remote::RemoteAdapterKind,
    };
    let config = configuration(Path::new(CONFIG))?;
    let mut results = Vec::new();
    for (name, provider) in &config.remote.providers {
        if selected.is_some_and(|selected| selected != name) {
            continue;
        }
        if provider.adapter != RemoteAdapterKind::Browserstack {
            return Err(io::Error::other("Unsupported remote provider"));
        }
        let adapter = UsageAdapter::Browserstack;
        if !adapter.authorizes_origin(&provider.endpoint.origin()) {
            return Err(io::Error::other("Unsupported remote provider origin"));
        }
        let authorization = config
            .authorization
            .get(name)
            .ok_or_else(|| io::Error::other("Remote credentials are unavailable"))?;
        let sample = fetch_usage(adapter, &adapter.endpoint(), authorization);
        let (running, allowed, queued, error) = match sample {
            Ok(sample) => (Some(sample.running), Some(sample.allowed), Some(sample.queued), None),
            Err(error) => (None, None, None, Some(error.to_string())),
        };
        results.push(
            horizon_browser_control::manifest::provider_usage::ProviderUsageSummary {
                provider: name.clone(),
                supported: true,
                local_session_limit: provider.local_session_limit(),
                running,
                allowed,
                queued,
                sampled_at_millis: error.is_none().then(horizon_browser_control::manifest::now_millis),
                error,
            },
        );
    }
    if results.is_empty() {
        return Err(io::Error::other("Remote provider is not configured"));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restarted_service_preserves_unknown_device_history_and_blocks_new_allocations() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("phone"),
            br#"{"provider":"account","reference":"retained-reference"}"#,
        )
        .unwrap();
        let history = Allocations::new(root.path().into()).unwrap();
        assert!(history.ensure_recoverable().is_err());
        assert_eq!(history.orphans["phone"].reference, "retained-reference");
        assert!(history.confirm_closed("phone").is_err());
        assert!(root.path().join("phone").is_file());
    }
    #[test]
    fn undeclared_target_fails_before_loading_credentials() {
        let capabilities: horizon_cloud::Capabilities = serde_json::from_str("{}").unwrap();
        assert!(
            request(
                &capabilities,
                "phone",
                &horizon_browser::provider_catalog::CatalogCache::default()
            )
            .unwrap_err()
            .to_string()
            .contains("disabled")
        );
    }
}
