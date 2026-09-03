//! Private, bounded host requests for creating browser panels.

use std::path::{Path, PathBuf};
use std::time::Duration;

use horizon_browser::{
    BackendKind, BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserControlAction, new_action_id,
    normalize_navigation_target,
};
use serde::{Deserialize, Serialize};

use super::request_queue::{
    MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json,
};
use super::workspace::AgentIdentity;
use super::{ManifestLock, actor_is_workspace_scoped};
use crate::horizon_home::{HorizonHome, safe_local_id};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowserCreateAuditStatus {
    Queued,
    Dispatched,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserCreateRequest {
    pub request_id: String,
    pub actor: String,
    /// The Horizon host that launched the requesting agent; only that host
    /// may claim the request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendKind>,
    #[serde(default = "default_visible")]
    pub visible: bool,
    pub requested_at_millis: i64,
    pub deadline_at_millis: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by_pid: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct BrowserCreateResult {
    pub request_id: String,
    pub actor: String,
    pub outcome: BrowserCreateOutcome,
}

/// Where the requested startup navigation stood when creation completed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateNavigation {
    /// No initial URL was requested; the panel is ready at its blank page.
    #[default]
    NotRequested,
    /// The requested page committed; the manifest URL is authoritative.
    Committed,
    /// The backend is ready but the requested page had not committed within
    /// the bounded startup wait; the panel is controllable and still loading.
    Pending,
    /// The backend is ready but the requested page failed to load; the panel
    /// is controllable at its previous (blank) document and
    /// `navigation_error` carries the browser's message.
    Failed,
    /// The user navigated the panel before the requested page committed; the
    /// panel is controllable at the user's page, and the requested page is
    /// not coming.
    Superseded,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BrowserCreateOutcome {
    Ready {
        panel_local_id: String,
        /// Absent in results written by hosts that predate startup
        /// readiness; readers must not treat that as proof that no URL was
        /// requested.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        navigation: Option<CreateNavigation>,
        /// The browser's message when `navigation` is `failed`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        navigation_error: Option<String>,
        /// Milliseconds from the host accepting the request until the panel
        /// was reported ready.
        #[serde(default)]
        startup_millis: u64,
    },
    Failed {
        code: String,
        message: String,
    },
}

impl BrowserCreateResult {
    #[must_use]
    pub fn ready(
        request: &BrowserCreateRequest,
        panel_local_id: String,
        navigation: CreateNavigation,
        navigation_error: Option<String>,
        startup_millis: u64,
    ) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            outcome: BrowserCreateOutcome::Ready {
                panel_local_id,
                navigation: Some(navigation),
                navigation_error,
                startup_millis,
            },
        }
    }

    #[must_use]
    pub fn failed(request: &BrowserCreateRequest, code: &str, message: &str) -> Self {
        Self {
            request_id: request.request_id.clone(),
            actor: request.actor.clone(),
            outcome: BrowserCreateOutcome::Failed {
                code: code.to_string(),
                message: message.to_string(),
            },
        }
    }
}

/// Queue a request that only the Horizon host that launched `identity` may
/// claim.
///
/// # Errors
/// Returns an error for a non-Horizon identity, a Horizon identity without a
/// forwarded host instance, an invalid URL, a full queue, or a private
/// coordination filesystem failure.
pub fn enqueue_create(
    identity: AgentIdentity<'_>,
    url: Option<String>,
    backend: Option<BackendKind>,
    visible: bool,
    timeout: Duration,
) -> std::io::Result<String> {
    enqueue_at(HorizonHome::resolve().root(), identity, url, backend, visible, timeout)
}

fn enqueue_at(
    root: &Path,
    identity: AgentIdentity<'_>,
    url: Option<String>,
    backend: Option<BackendKind>,
    visible: bool,
    timeout: Duration,
) -> std::io::Result<String> {
    let actor = identity.actor;
    super::agent::validate_actor(actor)?;
    if !actor_is_workspace_scoped(actor) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panels can be created only by an agent launched inside Horizon",
        ));
    }
    let Some(host_instance) = identity.host_instance else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panels can be created only when the launching Horizon host instance is known",
        ));
    };
    let url = url
        .map(|value| normalize_navigation_target(&value))
        .filter(|value| !value.is_empty());
    if let Some(url) = &url {
        BrowserControlAction::Navigate {
            url: url.clone(),
            wait: horizon_browser::NavigationWait::default(),
            timeout_millis: None,
        }
        .validate()
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
    }

    let directory = create_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    prune_at(&directory)?;
    if request_count(&directory)? >= MAX_PENDING_REQUESTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "browser panel create queue is full",
        ));
    }

    let request_id = new_action_id();
    let requested_at_millis = super::now_millis();
    let timeout_millis = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
    let request = BrowserCreateRequest {
        request_id: request_id.clone(),
        actor: actor.to_string(),
        host_instance: Some(host_instance.to_string()),
        url,
        backend,
        visible,
        requested_at_millis,
        deadline_at_millis: requested_at_millis.saturating_add(timeout_millis),
        claimed_by_pid: None,
    };
    write_private_json(&request_path(root, &request_id), &request)?;
    Ok(request_id)
}

/// List valid pending requests without exposing their private file paths.
///
/// # Errors
/// Returns an error when the request directory cannot be read.
pub fn list_create_requests() -> std::io::Result<Vec<BrowserCreateRequest>> {
    list_at(HorizonHome::resolve().root())
}

fn list_at(root: &Path) -> std::io::Result<Vec<BrowserCreateRequest>> {
    let directory = create_directory(root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let _queue_lock = match ManifestLock::acquire(&queue_lock_path(&directory)) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut requests = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        let Some(encoded_id) = file_name.strip_suffix(".request.json") else {
            continue;
        };
        let Some(request) = read_json::<BrowserCreateRequest>(&entry.path())? else {
            continue;
        };
        if safe_local_id(&request.request_id) == encoded_id && request.claimed_by_pid.is_none() {
            requests.push(request);
        }
    }
    requests.sort_by_key(|request| request.requested_at_millis);
    Ok(requests)
}

/// Atomically claim one request for this Horizon process when its actor still
/// matches the candidate agent panel and the request names this host.
///
/// # Errors
/// Returns an error for an identity mismatch, invalid data, or filesystem
/// failure. `Ok(None)` means another host already claimed the request or the
/// request belongs to a different Horizon host instance.
pub fn claim_create_request(
    request_id: &str,
    actor: &str,
    host_instance: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserCreateRequest>> {
    claim_at(
        HorizonHome::resolve().root(),
        request_id,
        actor,
        host_instance,
        claimant_pid,
    )
}

fn claim_at(
    root: &Path,
    request_id: &str,
    actor: &str,
    host_instance: &str,
    claimant_pid: u32,
) -> std::io::Result<Option<BrowserCreateRequest>> {
    let directory = create_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let _queue_lock = match ManifestLock::acquire(&queue_lock_path(&directory)) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let path = request_path(root, request_id);
    let Some(mut request) = read_json::<BrowserCreateRequest>(&path)? else {
        return Ok(None);
    };
    if request.request_id != request_id || request.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser create request identity did not match its path or actor",
        ));
    }
    // Compared under the queue lock: a second live host running a copy of
    // the same session shares the actor but never this host instance.
    if request.claimed_by_pid.is_some() || request.host_instance.as_deref() != Some(host_instance) {
        return Ok(None);
    }
    request.claimed_by_pid = Some(claimant_pid);
    write_private_json(&path, &request)?;
    Ok(Some(request))
}

/// Publish a terminal create result and remove its request.
///
/// # Errors
/// Returns an error when the private result cannot be written atomically.
pub fn complete_create_request(result: &BrowserCreateResult) -> std::io::Result<()> {
    complete_at(HorizonHome::resolve().root(), result)
}

fn complete_at(root: &Path, result: &BrowserCreateResult) -> std::io::Result<()> {
    let directory = create_directory(root);
    std::fs::create_dir_all(&directory)?;
    let _queue_lock = ManifestLock::acquire(&queue_lock_path(&directory))?;
    let result_path = result_path(root, &result.request_id);
    write_private_json(&result_path, result)?;

    let request_path = request_path(root, &result.request_id);
    match std::fs::remove_file(request_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Atomically consume a create result for the exact requesting actor.
///
/// # Errors
/// Returns an error for invalid data, an actor mismatch, or filesystem
/// failure. Invalid results are retained for diagnosis.
pub fn take_create_result(request_id: &str, actor: &str) -> std::io::Result<Option<BrowserCreateResult>> {
    take_at(HorizonHome::resolve().root(), request_id, actor)
}

fn take_at(root: &Path, request_id: &str, actor: &str) -> std::io::Result<Option<BrowserCreateResult>> {
    let directory = create_directory(root);
    if !directory.exists() {
        return Ok(None);
    }
    let path = result_path(root, request_id);
    if read_json::<BrowserCreateResult>(&path)?.is_none() {
        return Ok(None);
    }
    let _queue_lock = match ManifestLock::acquire(&queue_lock_path(&directory)) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(result) = read_json::<BrowserCreateResult>(&path)? else {
        return Ok(None);
    };
    if result.request_id != request_id || result.actor != actor {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "browser create result identity did not match its path or actor",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(Some(result)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Append one creation lifecycle state to the new panel's audit journal.
///
/// # Errors
/// Returns an error for an invalid actor or audit filesystem failure.
pub fn record_create_status(
    panel_local_id: &str,
    request: &BrowserCreateRequest,
    backend: BackendKind,
    status: BrowserCreateAuditStatus,
) -> std::io::Result<()> {
    super::agent::validate_actor(&request.actor)?;
    super::audit::append(
        &BrowserAuditEntry::new(
            request.request_id.clone(),
            BrowserAuditActor::Agent {
                name: request.actor.clone(),
            },
            match status {
                BrowserCreateAuditStatus::Queued => horizon_browser::BrowserAuditStatus::Queued,
                BrowserCreateAuditStatus::Dispatched => horizon_browser::BrowserAuditStatus::Dispatched,
                BrowserCreateAuditStatus::Completed => horizon_browser::BrowserAuditStatus::Completed,
                BrowserCreateAuditStatus::Failed => horizon_browser::BrowserAuditStatus::Failed,
            },
            BrowserAuditAction::session_created(backend, request.url.as_deref(), request.visible),
        ),
        panel_local_id,
    )
}

fn create_directory(root: &Path) -> PathBuf {
    root.join("runtime").join("browser-create")
}

fn request_path(root: &Path, request_id: &str) -> PathBuf {
    create_directory(root).join(format!("{}.request.json", safe_local_id(request_id)))
}

fn result_path(root: &Path, request_id: &str) -> PathBuf {
    create_directory(root).join(format!("{}.result.json", safe_local_id(request_id)))
}

const fn default_visible() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_outcomes_keep_the_legacy_shape_and_default_the_navigation_state() {
        let legacy: BrowserCreateOutcome =
            serde_json::from_value(serde_json::json!({ "status": "ready", "panel_local_id": "panel" }))
                .expect("legacy ready outcome decodes");
        assert_eq!(
            legacy,
            BrowserCreateOutcome::Ready {
                panel_local_id: "panel".to_string(),
                navigation: None,
                navigation_error: None,
                startup_millis: 0,
            },
            "a legacy result leaves the navigation state unknown rather than claiming no URL was requested"
        );
        let encoded = serde_json::to_value(BrowserCreateOutcome::Ready {
            panel_local_id: "panel".to_string(),
            navigation: Some(CreateNavigation::Pending),
            navigation_error: None,
            startup_millis: 1234,
        })
        .expect("encode");
        assert_eq!(encoded["navigation"], "pending");
        assert_eq!(encoded["startup_millis"], 1234);
        assert!(encoded.get("navigation_error").is_none());
        let failed = serde_json::to_value(BrowserCreateOutcome::Ready {
            panel_local_id: "panel".to_string(),
            navigation: Some(CreateNavigation::Failed),
            navigation_error: Some("could not navigate to https://down.test/".to_string()),
            startup_millis: 900,
        })
        .expect("encode");
        assert_eq!(failed["navigation"], "failed");
        assert_eq!(failed["navigation_error"], "could not navigate to https://down.test/");
    }

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().expect("isolated create root")
    }

    #[test]
    fn create_request_is_private_claimed_once_and_consumed_by_its_actor() {
        let root = root();
        let actor = "horizon:agent-panel";
        let identity = AgentIdentity::new(actor, Some("host-a"));
        let request_id = enqueue_at(
            root.path(),
            identity,
            Some("example.test/path".to_string()),
            Some(BackendKind::FirefoxBidi),
            false,
            Duration::from_secs(30),
        )
        .expect("enqueue create");
        let requests = list_at(root.path()).expect("list create requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.as_deref(), Some("https://example.test/path"));
        assert_eq!(requests[0].host_instance.as_deref(), Some("host-a"));
        assert!(!requests[0].visible);

        assert!(
            claim_at(root.path(), &request_id, actor, "host-b", 41)
                .expect("other host claim")
                .is_none(),
            "a host that did not launch the agent must not claim its request"
        );
        let claimed = claim_at(root.path(), &request_id, actor, "host-a", 42)
            .expect("claim create")
            .expect("unclaimed request");
        assert!(
            claim_at(root.path(), &request_id, actor, "host-a", 43)
                .expect("second claim")
                .is_none()
        );
        assert!(list_at(root.path()).expect("list claimed requests").is_empty());

        let result = BrowserCreateResult::ready(
            &claimed,
            "browser-panel".to_string(),
            CreateNavigation::Committed,
            None,
            1_500,
        );
        complete_at(root.path(), &result).expect("complete create");
        assert_eq!(
            take_at(root.path(), &request_id, actor).expect("take result"),
            Some(result)
        );
        assert!(take_at(root.path(), &request_id, actor).expect("take once").is_none());
        let per_request_locks = std::fs::read_dir(create_directory(root.path()))
            .expect("create directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.ends_with(".request.json.lock") || name.ends_with(".result.json.lock")
            })
            .count();
        assert_eq!(per_request_locks, 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let another =
                enqueue_at(root.path(), identity, None, None, true, Duration::from_secs(30)).expect("second request");
            assert_eq!(
                std::fs::metadata(request_path(root.path(), &another))
                    .expect("request metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn create_request_requires_a_horizon_actor_and_exact_result_identity() {
        let root = root();
        assert_eq!(
            enqueue_at(
                root.path(),
                AgentIdentity::new("external", Some("host-a")),
                None,
                None,
                true,
                Duration::from_secs(30),
            )
            .expect_err("external actor must fail")
            .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            enqueue_at(
                root.path(),
                AgentIdentity::new("horizon:agent-panel", None),
                None,
                None,
                true,
                Duration::from_secs(30),
            )
            .expect_err("a Horizon actor without a host instance must fail")
            .kind(),
            std::io::ErrorKind::PermissionDenied
        );

        let actor = "horizon:agent-panel";
        let request_id = enqueue_at(
            root.path(),
            AgentIdentity::new(actor, Some("host-a")),
            Some("http://127.0.0.1:3000".to_string()),
            None,
            true,
            Duration::from_secs(30),
        )
        .expect("enqueue create");
        let request = claim_at(root.path(), &request_id, actor, "host-a", 42)
            .expect("claim")
            .expect("request");
        assert_eq!(request.url.as_deref(), Some("http://127.0.0.1:3000"));
        complete_at(
            root.path(),
            &BrowserCreateResult::failed(&request, "unavailable", "backend unavailable"),
        )
        .expect("complete failure");
        assert_eq!(
            take_at(root.path(), &request_id, "horizon:other")
                .expect_err("wrong actor must not consume")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        assert!(take_at(root.path(), &request_id, actor).expect("right actor").is_some());
    }

    #[test]
    fn concurrent_enqueues_keep_the_host_queue_bounded() {
        let root = root();
        let root_path = std::sync::Arc::new(root.path().to_path_buf());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(MAX_PENDING_REQUESTS * 2));
        let mut workers = Vec::new();
        for index in 0..MAX_PENDING_REQUESTS * 2 {
            let root_path = std::sync::Arc::clone(&root_path);
            let barrier = std::sync::Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let actor = format!("horizon:agent-{index}");
                enqueue_at(
                    &root_path,
                    AgentIdentity::new(&actor, Some("host-a")),
                    None,
                    None,
                    true,
                    Duration::from_secs(30),
                )
            }));
        }
        let accepted = workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("enqueue worker"))
            .count();

        assert_eq!(accepted, MAX_PENDING_REQUESTS);
        assert_eq!(
            request_count(&create_directory(&root_path)).expect("request count"),
            MAX_PENDING_REQUESTS
        );
    }
}
