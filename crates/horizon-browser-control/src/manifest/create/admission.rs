//! Serialized admission of one agent's new browser panels.
//!
//! The MCP server decides whether a create needs `allow_additional` from the
//! agent's owned panels and its creates that are still queued or in
//! progress. The host stamps the owner into a new panel's manifest before it
//! removes the request, so under this lock every create of the agent is
//! visible as a request or as an owned panel. Holding the lock until the new
//! request is queued keeps concurrent creates, from one MCP server or from
//! several, from passing the same check. One lock serves every agent: it is
//! held only for a few file reads, and per-agent lock files would accumulate.

use std::path::Path;

use horizon_browser::BackendKind;

use super::super::request_queue::{queue_lock_path, read_json};
use super::super::workspace::AgentIdentity;
use super::super::{ManifestLock, now_millis};
use super::{BrowserCreateRequest, create_directory};
use crate::paths::BrowserRuntimePaths;

/// A create of the agent that is queued or in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingCreate {
    /// The engine it names; `None` when the host picks its configured browser.
    pub backend: Option<BackendKind>,
    /// It runs at a remote target.
    pub remote: bool,
}

/// The create admission lock and the agent's pending creates. Queue the new
/// request before this value is dropped.
pub struct CreateAdmission {
    _lock: ManifestLock,
    pending: Vec<PendingCreate>,
}

impl CreateAdmission {
    /// Creates of the agent that are queued or in progress and not past their
    /// deadline. Duplicates of an existing panel are not included.
    #[must_use]
    pub fn pending(&self) -> &[PendingCreate] {
        &self.pending
    }
}

/// Lock create admission and read the agent's pending creates.
///
/// # Errors
/// Returns an error for an invalid actor, a lock timeout, or a filesystem
/// failure.
pub fn admit_create(identity: AgentIdentity<'_>) -> std::io::Result<CreateAdmission> {
    admit_at(BrowserRuntimePaths::resolve().root(), identity, now_millis())
}

fn admit_at(root: &Path, identity: AgentIdentity<'_>, now: i64) -> std::io::Result<CreateAdmission> {
    super::super::agent::validate_actor(identity.actor)?;
    let directory = root.join("runtime");
    std::fs::create_dir_all(&directory)?;
    let lock = ManifestLock::acquire(&directory.join("browser-create-admission.json"))?;
    let mut pending = pending_at(root, identity, now)?;
    pending.extend(pending_at(&root.join("oriented-sessions"), identity, now)?);
    Ok(CreateAdmission { _lock: lock, pending })
}

fn pending_at(root: &Path, identity: AgentIdentity<'_>, now: i64) -> std::io::Result<Vec<PendingCreate>> {
    let directory = create_directory(root);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let _queue_lock = match ManifestLock::acquire(&queue_lock_path(&directory)) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut pending = Vec::new();
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().ends_with(".request.json") {
            continue;
        }
        // A request this version cannot read, for example one written by
        // another Horizon version, must not block every create.
        let request = match read_json::<BrowserCreateRequest>(&entry.path()) {
            Ok(Some(request)) => request,
            Ok(None) => continue,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                tracing::warn!(%error, "skipping an unreadable browser create request");
                continue;
            }
            Err(error) => return Err(error),
        };
        if request.actor == identity.actor
            && request.host_instance.as_deref() == identity.host_instance
            && request.deadline_at_millis >= now
            && request.duplicate_from.is_none()
        {
            pending.push(PendingCreate {
                backend: request.backend,
                remote: request.target.is_some(),
            });
        }
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::create::{CreateParameters, enqueue_operation_at};
    use std::time::Duration;

    const ACTOR: &str = "horizon:admission-agent";
    const HOST: &str = "admission-host";

    fn identity(actor: &str) -> AgentIdentity<'_> {
        AgentIdentity::new(actor, Some(HOST))
    }

    fn queue(root: &Path, actor: &str, backend: Option<BackendKind>, timeout: Duration) {
        enqueue_operation_at(
            root,
            identity(actor),
            CreateParameters {
                url: None,
                backend,
                target: None,
                orientation: None,
                visible: true,
                duplicate_from: None,
            },
            timeout,
        )
        .expect("queue create");
    }

    #[test]
    fn admission_lists_the_agents_live_creates_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert!(
            admit_at(root, identity(ACTOR), now_millis())
                .unwrap()
                .pending()
                .is_empty()
        );
        queue(root, ACTOR, Some(BackendKind::FirefoxBidi), Duration::from_mins(1));
        queue(root, ACTOR, None, Duration::from_millis(1));
        queue(
            root,
            "horizon:another-agent",
            Some(BackendKind::ChromiumCdp),
            Duration::from_mins(1),
        );
        std::fs::write(
            create_directory(root).join("other-version.request.json"),
            br#"{"request_id": 7}"#,
        )
        .unwrap();

        let later = now_millis() + 1_000;
        let admission = admit_at(root, identity(ACTOR), later).unwrap();
        assert_eq!(
            admission.pending(),
            [PendingCreate {
                backend: Some(BackendKind::FirefoxBidi),
                remote: false,
            }],
            "another agent's create, an expired create and an unreadable request do not count"
        );
    }

    #[test]
    fn admission_is_exclusive() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let held = admit_at(&root, identity(ACTOR), now_millis()).unwrap();
        let admitted = std::sync::Arc::new(AtomicBool::new(false));
        let contender = std::thread::spawn({
            let admitted = std::sync::Arc::clone(&admitted);
            move || {
                let admission = admit_at(&root, identity(ACTOR), now_millis());
                admitted.store(true, Ordering::SeqCst);
                admission.map(|_| ())
            }
        });
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !admitted.load(Ordering::SeqCst),
            "a second admission waits while the first is held"
        );
        drop(held);
        contender.join().unwrap().expect("admission after release");
        assert!(admitted.load(Ordering::SeqCst));
    }
}
