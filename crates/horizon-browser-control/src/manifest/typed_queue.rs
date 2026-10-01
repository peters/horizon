//! A bounded, host-bound request queue with typed operations and outcomes.
//!
//! New tool families use this instead of copying the queue mechanics: requests
//! and results are private files, requests are claimed only by the host that
//! injected the caller's identity, and a result is returned only to the exact
//! requesting actor.

use std::{
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{
    AgentIdentity, ManifestLock, now_millis,
    request_queue::{MAX_PENDING_REQUESTS, prune_at, queue_lock_path, read_json, request_count, write_private_json},
};
use crate::paths::safe_local_id;

/// One queued call, addressed to the host that issued the caller's identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Queued<Operation> {
    pub request_id: String,
    pub actor: String,
    pub host_instance: String,
    pub deadline_at_millis: i64,
    pub operation: Operation,
}

#[derive(Deserialize, Serialize)]
struct ResultEnvelope<Outcome> {
    actor: String,
    host_instance: String,
    outcome: Outcome,
}

/// A queue directory under the runtime root, with the label used in errors.
pub(super) struct TypedQueue {
    directory: &'static str,
    label: &'static str,
}

impl TypedQueue {
    pub(super) const fn new(directory: &'static str, label: &'static str) -> Self {
        Self { directory, label }
    }

    fn dir(&self, root: &Path) -> PathBuf {
        root.join("runtime").join(self.directory)
    }

    fn path(&self, root: &Path, id: &str, kind: &str) -> PathBuf {
        self.dir(root).join(format!("{}.{kind}.json", safe_local_id(id)))
    }

    pub(super) fn enqueue_at<Operation: Serialize>(
        &self,
        root: &Path,
        identity: AgentIdentity<'_>,
        operation: Operation,
        timeout: Duration,
    ) -> io::Result<Queued<Operation>> {
        super::agent::validate_actor(identity.actor)?;
        let host = identity.host_instance.filter(|host| super::valid_host_instance(host));
        if !identity.workspace_scoped() || host.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} requires an agent launched inside Horizon with its host identity",
                    self.label
                ),
            ));
        }
        let dir = self.dir(root);
        std::fs::create_dir_all(&dir)?;
        let _lock = ManifestLock::acquire(&queue_lock_path(&dir))?;
        prune_at(&dir)?;
        if request_count(&dir)? >= MAX_PENDING_REQUESTS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("{} request queue is full", self.label),
            ));
        }
        let request = Queued {
            request_id: horizon_browser::new_action_id(),
            actor: identity.actor.into(),
            host_instance: host.unwrap_or_default().into(),
            deadline_at_millis: now_millis().saturating_add(i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX)),
            operation,
        };
        write_private_json(&self.path(root, &request.request_id, "request"), &request)?;
        Ok(request)
    }

    /// Atomically claim only the requests addressed to `host`.
    pub(super) fn claim_at<Operation: DeserializeOwned>(
        &self,
        root: &Path,
        host: &str,
    ) -> io::Result<Vec<Queued<Operation>>> {
        let dir = self.dir(root);
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
            let Ok(Some(request)) = read_json::<Queued<Operation>>(&entry.path()) else {
                continue;
            };
            if request.host_instance != host || entry.path() != self.path(root, &request.request_id, "request") {
                continue;
            }
            std::fs::remove_file(entry.path())?;
            requests.push(request);
        }
        Ok(requests)
    }

    pub(super) fn complete_at<Operation, Outcome: Serialize>(
        &self,
        root: &Path,
        request: &Queued<Operation>,
        outcome: Outcome,
    ) -> io::Result<()> {
        write_private_json(
            &self.path(root, &request.request_id, "result"),
            &ResultEnvelope {
                actor: request.actor.clone(),
                host_instance: request.host_instance.clone(),
                outcome,
            },
        )
    }

    /// Consume only a result for the exact requesting actor and host.
    pub(super) fn take_result_at<Operation, Outcome: DeserializeOwned>(
        &self,
        root: &Path,
        request: &Queued<Operation>,
    ) -> io::Result<Option<Outcome>> {
        let path = self.path(root, &request.request_id, "result");
        let Some(result) = read_json::<ResultEnvelope<Outcome>>(&path)? else {
            return Ok(None);
        };
        if result.actor != request.actor || result.host_instance != request.host_instance {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} result identity mismatch", self.label),
            ));
        }
        std::fs::remove_file(path)?;
        Ok(Some(result.outcome))
    }
}
