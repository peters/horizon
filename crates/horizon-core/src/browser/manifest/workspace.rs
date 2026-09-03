//! Host-authoritative workspace membership for browser MCP authorization.
//!
//! Browser manifests are shared by every Horizon process using the same
//! Horizon home, so a panel id alone never proves that the calling agent may
//! control a panel. Only the Horizon host knows board membership, so it stamps
//! each live browser manifest with the agent identities currently sharing the
//! panel's workspace. The MCP adapter authorizes a workspace-scoped caller by
//! that membership on every call: moving either panel between workspaces
//! changes the stamp on the host's next sync without restarting the browser
//! session or the agent. The stamp also names the host process, and Horizon
//! injects that same host instance into every agent it launches, so two live
//! hosts sharing one home never authorize each other even when persisted
//! panel ids collide (duplicated, copied, or taken-over sessions). A manifest
//! without a stamp fails closed.

use std::path::Path;
use std::sync::OnceLock;

use horizon_browser::BrowserAuditEntry;
use serde::{Deserialize, Serialize};

use super::{
    BrowserManifest, ManifestLock, audit, default_manifest_path, manifest_path_for_root, mutate_at, now_millis,
    read_at, try_read_at,
};
use crate::horizon_home::HorizonHome;

/// Error text shared by every locked transaction that refuses an identity
/// outside the panel's workspace.
pub const OUTSIDE_WORKSPACE_MESSAGE: &str = "agent is outside this browser panel's workspace";

/// What a host-state sync did to one manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostStampOutcome {
    /// Presentation or membership changed and the manifest was rewritten.
    Written,
    /// The manifest already carried this host state; nothing was written.
    Unchanged,
    /// The manifest records another host's driver (or none), so this host
    /// must not stamp it. Distinct from an I/O failure, which is an error.
    NotOwned,
}
const OTHER_HOST_MESSAGE: &str = "browser panel's driver runs in another Horizon host";
/// Environment variable through which Horizon tells an agent process, and
/// the MCP server it starts, which host process injected its identity.
pub const HOST_INSTANCE_ENV: &str = "HORIZON_BROWSER_HOST_INSTANCE";
static HOST_INSTANCE: OnceLock<String> = OnceLock::new();

/// Identity of this Horizon host process, generated once per process.
#[must_use]
pub fn host_instance() -> &'static str {
    HOST_INSTANCE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

#[must_use]
pub fn valid_host_instance(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

/// Who is asking: the injected actor and the host process that injected it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentIdentity<'a> {
    pub actor: &'a str,
    /// `None` when the caller was not launched by a Horizon host or its
    /// launcher did not forward [`HOST_INSTANCE_ENV`]; a workspace-scoped
    /// identity without it can never satisfy a stamp.
    pub host_instance: Option<&'a str>,
}

impl<'a> AgentIdentity<'a> {
    #[must_use]
    pub const fn new(actor: &'a str, host_instance: Option<&'a str>) -> Self {
        Self { actor, host_instance }
    }

    #[must_use]
    pub fn workspace_scoped(&self) -> bool {
        actor_is_workspace_scoped(self.actor)
    }
}

/// The workspace a browser panel currently belongs to, as its host sees it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestWorkspace {
    /// The host process that stamped the panel; only agents that process
    /// launched can match.
    #[serde(default)]
    pub host_instance: String,
    /// Persisted local id of the workspace, for diagnostics only.
    pub local_id: String,
    /// Sorted, de-duplicated identities of the agent panels in that
    /// workspace: the only workspace-scoped callers allowed to discover or
    /// control the panel.
    #[serde(default)]
    pub actors: Vec<String>,
}

/// Whether `actor` is a Horizon agent identity whose access the host decides
/// through the workspace stamp. Other identities (standalone hosts, the
/// process-local fallback) are not placed anywhere and stay unscoped.
#[must_use]
pub fn actor_is_workspace_scoped(actor: &str) -> bool {
    actor
        .strip_prefix("horizon:")
        .is_some_and(|identity| !identity.is_empty())
}

/// The guard every privileged manifest transaction evaluates while holding
/// the manifest lock, so a concurrent host re-stamp cannot slip an operation
/// past the workspace boundary.
///
/// # Errors
/// Returns `PermissionDenied` when the manifest does not permit `identity`.
pub(super) fn permit(manifest: &BrowserManifest, identity: AgentIdentity<'_>) -> std::io::Result<()> {
    if manifest.permits(identity) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OUTSIDE_WORKSPACE_MESSAGE,
        ))
    }
}

/// Read a panel's audit journal only when `identity` may control the panel,
/// checking membership and reading under the manifest lock.
///
/// # Errors
/// Returns `NotFound` for a panel that is not live, `PermissionDenied` for an
/// identity outside its workspace, or an audit storage failure.
pub fn read_audit_for(panel_local_id: &str, identity: AgentIdentity<'_>) -> std::io::Result<Vec<BrowserAuditEntry>> {
    Ok(read_audit_journal_for(panel_local_id, identity)?.entries)
}

/// Read retained audit records and loss counters when `identity` may control
/// the panel, checking membership and reading under the manifest lock.
///
/// # Errors
/// Returns `NotFound` for a panel that is not live, `PermissionDenied` for an
/// identity outside its workspace, or an audit storage failure.
pub fn read_audit_journal_for(
    panel_local_id: &str,
    identity: AgentIdentity<'_>,
) -> std::io::Result<audit::AuditJournal> {
    read_audit_journal_for_at(HorizonHome::resolve().root(), panel_local_id, identity)
}

fn read_audit_journal_for_at(
    root: &Path,
    panel_local_id: &str,
    identity: AgentIdentity<'_>,
) -> std::io::Result<audit::AuditJournal> {
    let manifest_path = manifest_path_for_root(root, panel_local_id);
    let _lock = ManifestLock::acquire(&manifest_path)?;
    let manifest = read_at(&manifest_path)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "browser panel is not live"))?;
    permit(&manifest, identity)?;
    audit::read_journal_at(&audit::audit_path_for_root(root, panel_local_id))
}

impl ManifestWorkspace {
    #[must_use]
    pub fn new(host_instance: &str, local_id: &str, mut actors: Vec<String>) -> Self {
        actors.sort();
        actors.dedup();
        Self {
            host_instance: host_instance.to_string(),
            local_id: local_id.to_string(),
            actors,
        }
    }

    /// Whether `identity` was launched by the stamping host and sits in this
    /// workspace. The host check makes identical persisted panel ids in two
    /// live hosts (duplicated or copied sessions) mutually invisible.
    #[must_use]
    pub fn authorizes(&self, identity: AgentIdentity<'_>) -> bool {
        identity.host_instance == Some(self.host_instance.as_str())
            && self.actors.iter().any(|candidate| candidate == identity.actor)
    }
}

/// Stamp host-owned presentation and placement state on a live manifest,
/// writing only when something differs.
///
/// # Errors
/// Returns `NotFound` when the panel is not live, or another error when the
/// locked manifest transaction fails. A manifest owned by another host's
/// driver is reported as [`HostStampOutcome::NotOwned`], not as an error.
pub fn sync_host_state(
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
) -> std::io::Result<HostStampOutcome> {
    sync_host_state_at(
        &default_manifest_path(panel_local_id),
        panel_local_id,
        visible,
        workspace,
    )
}

/// Like [`sync_host_state`], addressing the manifest under `root` instead of
/// the process default Horizon home.
///
/// # Errors
/// Same as [`sync_host_state`].
pub fn sync_host_state_in(
    root: &Path,
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
) -> std::io::Result<HostStampOutcome> {
    sync_host_state_at(
        &manifest_path_for_root(root, panel_local_id),
        panel_local_id,
        visible,
        workspace,
    )
}

fn sync_host_state_at(
    path: &Path,
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
) -> std::io::Result<HostStampOutcome> {
    let hidden = !visible;
    // Only an absent file is "not live"; a read or parse failure propagates
    // so callers that retry later do not mistake it for a missing panel.
    let current = try_read_at(path)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("browser manifest is not live: {}", path.display()),
        )
    })?;
    if !driver_host_matches(&current, workspace) {
        return Ok(HostStampOutcome::NotOwned);
    }
    if current.hidden == hidden && current.workspace.as_ref() == Some(workspace) {
        return Ok(HostStampOutcome::Unchanged);
    }
    let mut outcome = HostStampOutcome::NotOwned;
    mutate_at(path, panel_local_id, false, |manifest| {
        if driver_host_matches(manifest, workspace) {
            stamp(manifest, hidden, workspace, now_millis());
            outcome = HostStampOutcome::Written;
            true
        } else {
            false
        }
    })?;
    Ok(outcome)
}

/// Stamp a freshly created panel and assign its requested owner in one locked
/// transaction, so no other same-workspace agent can claim it in between.
///
/// # Errors
/// Returns `NotFound` when the panel is not live, `PermissionDenied` when the
/// driver runs in another host, the owner is outside the stamped workspace,
/// or another live owner already holds the panel.
pub fn publish_requested_panel(
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
    owner: AgentIdentity<'_>,
) -> std::io::Result<()> {
    publish_requested_panel_at(
        &default_manifest_path(panel_local_id),
        panel_local_id,
        visible,
        workspace,
        owner,
    )
}

fn publish_requested_panel_at(
    path: &Path,
    panel_local_id: &str,
    visible: bool,
    workspace: &ManifestWorkspace,
    owner: AgentIdentity<'_>,
) -> std::io::Result<()> {
    super::agent::validate_actor(owner.actor)?;
    let now = now_millis();
    let mut outcome = Ok(());
    mutate_at(path, panel_local_id, false, |manifest| {
        outcome = stamp_and_claim(manifest, !visible, workspace, owner, now);
        outcome.is_ok()
    })?;
    outcome
}

/// Validate the driver host, the requester's membership in the proposed
/// stamp, and the lease before mutating anything, so a refused request leaves
/// presentation, placement, and ownership untouched.
fn stamp_and_claim(
    manifest: &mut BrowserManifest,
    hidden: bool,
    workspace: &ManifestWorkspace,
    owner: AgentIdentity<'_>,
    now: i64,
) -> std::io::Result<()> {
    require_driver_host(manifest, workspace)?;
    if owner.workspace_scoped() && !workspace.authorizes(owner) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OUTSIDE_WORKSPACE_MESSAGE,
        ));
    }
    if manifest
        .live_owner(now)
        .is_some_and(|current| current.name != owner.actor)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel already has another live owner",
        ));
    }
    stamp(manifest, hidden, workspace, now);
    if super::agent::try_claim_owner(manifest, owner.actor, None, now) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel already has another live owner",
        ))
    }
}

fn stamp(manifest: &mut BrowserManifest, hidden: bool, workspace: &ManifestWorkspace, now: i64) {
    manifest.hidden = hidden;
    manifest.workspace = Some(workspace.clone());
    // An owner the new placement no longer permits loses its lease and any
    // pending handoff at once: handoffs have no TTL and would otherwise block
    // the destination workspace's agents until a manual hand-back.
    let owner_revoked = manifest.owner.as_ref().is_some_and(|owner| {
        let identity = AgentIdentity::new(&owner.name, manifest.host.as_deref());
        identity.workspace_scoped() && !workspace.authorizes(identity)
    });
    if owner_revoked {
        manifest.owner = None;
        manifest.handoff = None;
    }
    manifest.updated_at = now;
}

fn driver_host_matches(manifest: &BrowserManifest, workspace: &ManifestWorkspace) -> bool {
    manifest.host.as_deref() == Some(workspace.host_instance.as_str())
}

/// Only the process running the panel's driver may stamp it; a manifest
/// without a recorded host (older driver) is never stamped.
fn require_driver_host(manifest: &BrowserManifest, workspace: &ManifestWorkspace) -> std::io::Result<()> {
    if driver_host_matches(manifest, workspace) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OTHER_HOST_MESSAGE,
        ))
    }
}

#[cfg(test)]
mod tests;
