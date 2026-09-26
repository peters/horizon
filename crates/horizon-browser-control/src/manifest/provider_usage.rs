//! Bounded host-scoped, read-only requests for shared provider usage.
use super::request_queue::{MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, write_private_json};
use super::{AgentIdentity, ManifestLock};
use crate::paths::{BrowserRuntimePaths, safe_local_id};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderUsageSummary {
    pub provider: String,
    pub supported: bool,
    pub local_session_limit: Option<u32>,
    pub running: Option<u64>,
    pub allowed: Option<u64>,
    pub queued: Option<u64>,
    pub sampled_at_millis: Option<i64>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageRequest {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub provider: Option<String>,
    pub deadline_at_millis: i64,
    #[serde(default)]
    pub catalog: Option<horizon_browser::provider_catalog::CatalogQuery>,
    /// Requirements for ranked cloud compute offers, validated by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_offers: Option<serde_json::Value>,
    claimed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UsageResult {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub providers: Vec<ProviderUsageSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<horizon_browser::provider_catalog::CatalogPage>,
    /// Ranked cloud compute offers with the time the host observed their prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offers: Option<serde_json::Value>,
    pub error: Option<String>,
}

impl UsageRequest {
    #[must_use]
    pub fn result(&self, providers: Vec<ProviderUsageSummary>, error: Option<String>) -> UsageResult {
        UsageResult {
            request_id: self.request_id.clone(),
            actor: self.actor.clone(),
            host_instance: self.host_instance.clone(),
            providers,
            catalog: None,
            offers: None,
            error,
        }
    }
}

/// A usage queue bound to one private coordination root.
pub struct UsageQueue {
    root: PathBuf,
}

impl Default for UsageQueue {
    fn default() -> Self {
        Self::new(BrowserRuntimePaths::resolve().root().to_path_buf())
    }
}

impl UsageQueue {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// # Errors
    /// Invalid identity, unavailable storage, or a full queue.
    pub fn enqueue(&self, identity: AgentIdentity<'_>, provider: Option<String>) -> std::io::Result<String> {
        enqueue_at(&self.root, identity, provider)
    }

    /// # Errors
    /// Unavailable or malformed queue storage.
    pub fn claim(&self, host: &str) -> std::io::Result<Vec<UsageRequest>> {
        claim_at(&self.root, host)
    }

    /// # Errors
    /// Unavailable storage or a mismatched request identity.
    pub fn complete(&self, result: &UsageResult) -> std::io::Result<()> {
        complete_at(&self.root, result)
    }

    /// # Errors
    /// Unavailable storage or a mismatched result identity.
    pub fn take(&self, identity: AgentIdentity<'_>, request_id: &str) -> std::io::Result<Option<UsageResult>> {
        take_at(&self.root, identity, request_id)
    }
}

/// # Errors
/// Invalid host identity, a full queue, or unavailable private storage.
pub fn enqueue_provider_usage(identity: AgentIdentity<'_>, provider: Option<String>) -> std::io::Result<String> {
    UsageQueue::default().enqueue(identity, provider)
}

/// # Errors
/// Invalid discovery query, host identity, full queue or unavailable private storage.
pub fn enqueue_catalog(
    identity: AgentIdentity<'_>,
    query: horizon_browser::provider_catalog::CatalogQuery,
) -> std::io::Result<String> {
    enqueue_query_at(
        BrowserRuntimePaths::resolve().root(),
        identity,
        Some(query.provider.clone()),
        Some(query),
    )
}

/// Queues a read-only request for ranked cloud compute offers, answered by the live host
/// from prices it observed; nothing is rented.
/// # Errors
/// Invalid host identity, oversized requirements, a full queue or unavailable private storage.
pub fn enqueue_cloud_offers(identity: AgentIdentity<'_>, requirements: serde_json::Value) -> std::io::Result<String> {
    enqueue_offers_at(BrowserRuntimePaths::resolve().root(), identity, requirements)
}

fn enqueue_offers_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    requirements: serde_json::Value,
) -> std::io::Result<String> {
    if !requirements.is_object() || serde_json::to_vec(&requirements).map_or(true, |bytes| bytes.len() > 4096) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid cloud offer requirements",
        ));
    }
    enqueue_request_at(root, identity, None, None, Some(requirements))
}

fn enqueue_at(root: &Path, identity: AgentIdentity<'_>, provider: Option<String>) -> std::io::Result<String> {
    enqueue_query_at(root, identity, provider, None)
}

fn enqueue_query_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    provider: Option<String>,
    catalog: Option<horizon_browser::provider_catalog::CatalogQuery>,
) -> std::io::Result<String> {
    enqueue_request_at(root, identity, provider, catalog, None)
}

fn enqueue_request_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    provider: Option<String>,
    catalog: Option<horizon_browser::provider_catalog::CatalogQuery>,
    cloud_offers: Option<serde_json::Value>,
) -> std::io::Result<String> {
    if catalog.as_ref().is_some_and(|q| !q.valid()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid provider catalog query",
        ));
    }
    super::agent::validate_actor(identity.actor)?;
    let host = identity.host_instance.filter(|host| super::valid_host_instance(host));
    if !identity.workspace_scoped()
        || host.is_none()
        || provider.as_ref().is_some_and(|r| {
            r.is_empty()
                || r.len() > 64
                || !r
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "provider usage requires a Horizon host identity and a valid configured provider name",
        ));
    }
    let dir = directory(root);
    std::fs::create_dir_all(&dir)?;
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    if retained_entry_count(&dir)? >= MAX_PENDING_REQUESTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "provider_usage queue is full",
        ));
    }
    let request = UsageRequest {
        request_id: horizon_browser::new_action_id(),
        actor: identity.actor.to_string(),
        host_instance: host.unwrap_or_default().to_string(),
        provider,
        deadline_at_millis: super::now_millis() + 15_000,
        claimed: false,
        catalog,
        cloud_offers,
    };
    write_private_json(&path(root, &request.request_id, "request"), &request)?;
    Ok(request.request_id)
}

fn retained_entry_count(dir: &Path) -> std::io::Result<usize> {
    std::fs::read_dir(dir)?.try_fold(0, |count, entry| {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        Ok(count + usize::from(name.ends_with(".request.json") || name.ends_with(".result.json")))
    })
}

/// Claim only this host's requests atomically. Host dispatch must verify the
/// live actor and the caller's live membership before any disclosure.
/// # Errors
/// Unavailable or malformed queue storage.
pub fn claim_provider_usage_requests(host: &str) -> std::io::Result<Vec<UsageRequest>> {
    UsageQueue::default().claim(host)
}

fn claim_at(root: &Path, host: &str) -> std::io::Result<Vec<UsageRequest>> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    prune_at(&dir)?;
    let mut requests = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().ends_with(".request.json") {
            continue;
        }
        let Some(mut request) = read_json::<UsageRequest>(&entry.path())? else {
            continue;
        };
        if request.host_instance != host
            || request.claimed
            || entry.path() != path(root, &request.request_id, "request")
        {
            continue;
        }
        request.claimed = true;
        write_private_json(&entry.path(), &request)?;
        requests.push(request);
    }
    Ok(requests)
}

/// # Errors
/// Private result storage is unavailable.
pub fn complete_provider_usage(result: &UsageResult) -> std::io::Result<()> {
    UsageQueue::default().complete(result)
}

fn complete_at(root: &Path, result: &UsageResult) -> std::io::Result<()> {
    let dir = directory(root);
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    let request_path = path(root, &result.request_id, "request");
    let Some(request) = read_json::<UsageRequest>(&request_path)? else {
        return Ok(());
    };
    if !request.claimed || request.actor != result.actor || request.host_instance != result.host_instance {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "provider_usage result identity mismatch",
        ));
    }
    write_private_json(&path(root, &result.request_id, "result"), result)?;
    std::fs::remove_file(request_path)
}

/// # Errors
/// Result storage is unavailable or does not match the caller's identity.
pub fn take_provider_usage_result(
    identity: AgentIdentity<'_>,
    request_id: &str,
) -> std::io::Result<Option<UsageResult>> {
    UsageQueue::default().take(identity, request_id)
}

fn take_at(root: &Path, identity: AgentIdentity<'_>, request_id: &str) -> std::io::Result<Option<UsageResult>> {
    let dir = directory(root);
    if !dir.exists() {
        return Ok(None);
    }
    let result_path = path(root, request_id, "result");
    if !result_path.exists() {
        return Ok(None);
    }
    let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
    let Some(result) = read_json::<UsageResult>(&result_path)? else {
        return Ok(None);
    };
    if result.request_id != request_id
        || result.actor != identity.actor
        || Some(result.host_instance.as_str()) != identity.host_instance
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "provider_usage result identity mismatch",
        ));
    }
    std::fs::remove_file(result_path)?;
    Ok(Some(result))
}

fn directory(root: &Path) -> PathBuf {
    root.join("runtime").join("browser-provider-usage")
}
fn path(root: &Path, id: &str, kind: &str) -> PathBuf {
    directory(root).join(format!("{}.{kind}.json", safe_local_id(id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_queries_keep_host_actor_and_page_boundaries() {
        use horizon_browser::provider_catalog::{CatalogPage, CatalogQuery};
        let root = tempfile::tempdir().unwrap();
        let identity = AgentIdentity::new("horizon:agent", Some("host-a"));
        let query = CatalogQuery {
            provider: "account".into(),
            search: "phone".into(),
            offset: 50,
        };
        let id = enqueue_query_at(root.path(), identity, Some("account".into()), Some(query.clone())).unwrap();
        assert!(claim_at(root.path(), "host-b").unwrap().is_empty());
        let request = claim_at(root.path(), "host-a").unwrap().remove(0);
        assert_eq!(request.catalog, Some(query));
        let mut result = request.result(vec![], None);
        result.catalog = Some(CatalogPage {
            total: 51,
            ..Default::default()
        });
        complete_at(root.path(), &result).unwrap();
        assert!(take_at(root.path(), AgentIdentity::new("horizon:other", Some("host-a")), &id).is_err());
        assert_eq!(
            take_at(root.path(), identity, &id)
                .unwrap()
                .unwrap()
                .catalog
                .unwrap()
                .total,
            51
        );
        let invalid = CatalogQuery {
            provider: "account".into(),
            search: "x".repeat(129),
            offset: 0,
        };
        assert!(enqueue_query_at(root.path(), identity, Some("account".into()), Some(invalid)).is_err());
    }

    #[test]
    fn cloud_offer_requests_carry_their_requirements_and_reject_non_objects() {
        let root = tempfile::tempdir().unwrap();
        let identity = AgentIdentity::new("horizon:agent", Some("host-a"));
        let requirements = serde_json::json!({"gpu": true, "max_hourly": 0.5});
        let id = enqueue_offers_at(root.path(), identity, requirements.clone()).unwrap();
        assert!(claim_at(root.path(), "host-b").unwrap().is_empty());
        let request = claim_at(root.path(), "host-a").unwrap().remove(0);
        assert_eq!(
            (request.cloud_offers.as_ref(), request.catalog.is_none()),
            (Some(&requirements), true)
        );
        let mut result = request.result(Vec::new(), None);
        result.offers = Some(serde_json::json!({"offers": []}));
        complete_at(root.path(), &result).unwrap();
        let taken = take_at(root.path(), identity, &id).unwrap().unwrap();
        assert_eq!(taken.offers, Some(serde_json::json!({"offers": []})));
        for invalid in [serde_json::json!([1]), serde_json::json!({"text": "x".repeat(5000)})] {
            assert!(enqueue_offers_at(root.path(), identity, invalid).is_err());
        }
        assert!(enqueue_offers_at(root.path(), AgentIdentity::new("horizon:agent", None), requirements).is_err());
    }

    #[test]
    fn usage_requests_are_host_scoped_bounded_and_results_are_actor_scoped() {
        let root = tempfile::tempdir().expect("root");
        let queue = UsageQueue::new(root.path().to_path_buf());
        let identity = AgentIdentity::new("horizon:agent", Some("host-a"));
        for (actor, host, provider) in [
            ("external", Some("host-a"), None),
            ("horizon:agent", None, None),
            ("horizon:agent", Some("host-a"), Some("../secret".to_string())),
        ] {
            assert!(queue.enqueue(AgentIdentity::new(actor, host), provider).is_err());
        }
        let id = queue.enqueue(identity, Some("account-a".into())).expect("enqueue");
        assert!(queue.claim("host-b").expect("other host").is_empty());
        let requests = queue.claim("host-a").expect("claim");
        assert_eq!(requests.len(), 1);
        assert!(queue.claim("host-a").expect("claimed once").is_empty());
        assert_eq!(requests[0].provider.as_deref(), Some("account-a"));
        let result = requests[0].result(vec![], None);
        queue.complete(&result).expect("complete");
        assert!(
            queue
                .take(AgentIdentity::new("horizon:other", Some("host-a")), &id)
                .is_err()
        );
        assert!(queue.take(identity, &id).expect("own result").is_some());
        assert!(queue.take(identity, &id).expect("consume once").is_none());
        for _ in 0..MAX_PENDING_REQUESTS {
            queue.enqueue(identity, None).expect("bounded request");
        }
        assert_eq!(
            queue.enqueue(identity, None).expect_err("full").kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn unconsumed_results_remain_bounded_until_taken() {
        let root = tempfile::tempdir().expect("root");
        let queue = UsageQueue::new(root.path().to_path_buf());
        let identity = AgentIdentity::new("horizon:agent", Some("host-a"));
        for _ in 0..MAX_PENDING_REQUESTS {
            queue.enqueue(identity, None).expect("enqueue");
        }
        let requests = queue.claim("host-a").expect("claim");
        for request in &requests {
            queue.complete(&request.result(vec![], None)).expect("complete");
        }
        assert_eq!(
            queue
                .enqueue(identity, None)
                .expect_err("retained results fill queue")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        assert!(queue.take(identity, &requests[0].request_id).expect("take").is_some());
        queue.enqueue(identity, None).expect("one available slot");
        assert_eq!(
            queue.enqueue(identity, None).expect_err("full again").kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
