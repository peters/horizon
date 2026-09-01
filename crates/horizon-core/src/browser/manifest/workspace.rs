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
    BrowserManifest, ManifestLock, audit, default_manifest_path, manifest_path_for_root, now_millis, read_at, update_at,
};
use crate::horizon_home::HorizonHome;

/// Error text shared by every locked transaction that refuses an identity
/// outside the panel's workspace.
pub const OUTSIDE_WORKSPACE_MESSAGE: &str = "agent is outside this browser panel's workspace";
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
    read_audit_for_at(HorizonHome::resolve().root(), panel_local_id, identity)
}

fn read_audit_for_at(
    root: &Path,
    panel_local_id: &str,
    identity: AgentIdentity<'_>,
) -> std::io::Result<Vec<BrowserAuditEntry>> {
    let manifest_path = manifest_path_for_root(root, panel_local_id);
    let _lock = ManifestLock::acquire(&manifest_path)?;
    let manifest = read_at(&manifest_path)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "browser panel is not live"))?;
    permit(&manifest, identity)?;
    audit::read_at(&audit::audit_path_for_root(root, panel_local_id))
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
/// writing only when something differs. Returns whether a write happened.
///
/// # Errors
/// Returns `NotFound` when the panel is not live, or another error when the
/// locked manifest transaction fails.
pub fn sync_host_state(panel_local_id: &str, visible: bool, workspace: &ManifestWorkspace) -> std::io::Result<bool> {
    sync_host_state_at(
        &default_manifest_path(panel_local_id),
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
) -> std::io::Result<bool> {
    let hidden = !visible;
    let current = read_at(path).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("browser manifest is not live: {}", path.display()),
        )
    })?;
    require_driver_host(&current, workspace)?;
    if current.hidden == hidden && current.workspace.as_ref() == Some(workspace) {
        return Ok(false);
    }
    let mut outcome = Ok(());
    update_at(path, panel_local_id, |manifest| {
        outcome = require_driver_host(manifest, workspace);
        if outcome.is_ok() {
            stamp(manifest, hidden, workspace, now_millis());
        }
    })?;
    outcome.map(|()| true)
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
    update_at(path, panel_local_id, |manifest| {
        outcome = stamp_and_claim(manifest, !visible, workspace, owner, now);
    })?;
    outcome
}

fn stamp_and_claim(
    manifest: &mut BrowserManifest,
    hidden: bool,
    workspace: &ManifestWorkspace,
    owner: AgentIdentity<'_>,
    now: i64,
) -> std::io::Result<()> {
    require_driver_host(manifest, workspace)?;
    stamp(manifest, hidden, workspace, now);
    permit(manifest, owner)?;
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
    manifest.updated_at = now;
}

/// Only the process running the panel's driver may stamp it; a manifest
/// without a recorded host (older driver) is never stamped.
fn require_driver_host(manifest: &BrowserManifest, workspace: &ManifestWorkspace) -> std::io::Result<()> {
    if manifest.host.as_deref() == Some(workspace.host_instance.as_str()) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            OTHER_HOST_MESSAGE,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manifest::write_at;

    const HOST_A: &str = "host-a";
    const HOST_B: &str = "host-b";

    fn member() -> AgentIdentity<'static> {
        AgentIdentity::new("horizon:agent-a", Some(HOST_A))
    }

    fn stamp(host: &str, actors: &[&str]) -> ManifestWorkspace {
        ManifestWorkspace::new(host, "ws-a", actors.iter().map(|actor| (*actor).to_string()).collect())
    }

    #[test]
    fn only_horizon_agent_identities_are_workspace_scoped() {
        assert!(actor_is_workspace_scoped("horizon:agent-a"));
        assert!(!actor_is_workspace_scoped("horizon:"));
        assert!(!actor_is_workspace_scoped("horizon-mcp:4242"));
        assert!(!actor_is_workspace_scoped("browser-cli-test"));
        assert!(valid_host_instance(host_instance()));
        assert_eq!(host_instance(), host_instance(), "one identity per process");
        assert!(!valid_host_instance(""));
        assert!(!valid_host_instance("bad\ninstance"));

        let mut manifest = BrowserManifest::default();
        let external = AgentIdentity::new("browser-cli-test", None);
        assert!(manifest.permits(external), "unscoped identities need no stamp");
        assert!(!manifest.permits(member()), "unstamped manifests fail closed");
        assert_eq!(
            permit(&manifest, member()).expect_err("outside workspace").kind(),
            std::io::ErrorKind::PermissionDenied
        );
        manifest.workspace = Some(stamp(HOST_A, &["horizon:agent-a"]));
        assert!(manifest.permits(member()));
        assert!(!manifest.permits(AgentIdentity::new("horizon:agent-b", Some(HOST_A))));
        assert!(
            !manifest.permits(AgentIdentity::new("horizon:agent-a", Some(HOST_B))),
            "the same persisted actor id under another live host is a different agent"
        );
        assert!(
            !manifest.permits(AgentIdentity::new("horizon:agent-a", None)),
            "a Horizon agent without a forwarded host instance never matches"
        );
        permit(&manifest, member()).expect("member is permitted");
    }

    #[test]
    fn audit_reads_are_gated_by_workspace_membership_under_the_manifest_lock() {
        let root = tempfile::tempdir().expect("isolated root");
        let panel = "panel";
        write_at(
            &manifest_path_for_root(root.path(), panel),
            &BrowserManifest {
                panel_local_id: panel.to_string(),
                workspace: Some(stamp(HOST_A, &["horizon:agent-a"])),
                ..BrowserManifest::default()
            },
        )
        .expect("write manifest");
        audit::append_at_path(
            &audit::audit_path_for_root(root.path(), panel),
            &BrowserAuditEntry::new(
                "action-1".to_string(),
                horizon_browser::BrowserAuditActor::Agent {
                    name: "horizon:agent-a".to_string(),
                },
                horizon_browser::BrowserAuditStatus::Completed,
                horizon_browser::BrowserAuditAction::Reload,
            ),
        )
        .expect("append audit");

        let entries = read_audit_for_at(root.path(), panel, member()).expect("member reads audit");
        assert_eq!(entries.len(), 1);
        for outsider in [
            AgentIdentity::new("horizon:agent-b", Some(HOST_A)),
            AgentIdentity::new("horizon:agent-a", Some(HOST_B)),
            AgentIdentity::new("horizon:agent-a", None),
        ] {
            assert_eq!(
                read_audit_for_at(root.path(), panel, outsider)
                    .expect_err("non-member is refused")
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        assert_eq!(
            read_audit_for_at(root.path(), panel, AgentIdentity::new("browser-cli-test", None))
                .expect("unscoped identity reads audit")
                .len(),
            1
        );
        assert_eq!(
            read_audit_for_at(root.path(), "missing", member())
                .expect_err("missing panel")
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn membership_is_exact_and_an_unstamped_manifest_authorizes_nobody() {
        let workspace = ManifestWorkspace::new(
            HOST_A,
            "ws-a",
            vec![
                "horizon:agent-b".to_string(),
                "horizon:agent-a".to_string(),
                "horizon:agent-a".to_string(),
            ],
        );
        assert_eq!(workspace.actors, ["horizon:agent-a", "horizon:agent-b"]);
        assert!(workspace.authorizes(member()));
        assert!(!workspace.authorizes(AgentIdentity::new("horizon:agent-c", Some(HOST_A))));
        assert!(!workspace.authorizes(AgentIdentity::new("horizon:agent", Some(HOST_A))));

        let mut manifest = BrowserManifest::default();
        assert!(!manifest.authorizes(member()));
        manifest.workspace = Some(workspace);
        assert!(manifest.authorizes(member()));
        assert!(!manifest.authorizes(AgentIdentity::new("horizon:agent-c", Some(HOST_A))));
    }

    fn driver_manifest(host: Option<&str>) -> BrowserManifest {
        BrowserManifest {
            panel_local_id: "panel".to_string(),
            host: host.map(str::to_string),
            ..BrowserManifest::default()
        }
    }

    #[test]
    fn host_state_sync_writes_only_when_presentation_or_membership_changes() {
        let root = tempfile::tempdir().expect("isolated root");
        let path = manifest_path_for_root(root.path(), "panel");
        write_at(&path, &driver_manifest(Some(HOST_A))).expect("write manifest");
        let workspace = stamp(HOST_A, &["horizon:agent-a"]);

        assert!(sync_host_state_at(&path, "panel", true, &workspace).expect("stamp"));
        let stamped = read_at(&path).expect("read stamped");
        assert!(!stamped.hidden);
        assert_eq!(stamped.workspace.as_ref(), Some(&workspace));
        assert!(!sync_host_state_at(&path, "panel", true, &workspace).expect("steady state"));

        assert!(sync_host_state_at(&path, "panel", false, &workspace).expect("hide"));
        assert!(read_at(&path).expect("read hidden").hidden);

        let moved = ManifestWorkspace::new(HOST_A, "ws-b", vec!["horizon:agent-b".to_string()]);
        assert!(sync_host_state_at(&path, "panel", false, &moved).expect("move"));
        let after_move = read_at(&path).expect("read moved");
        assert!(after_move.authorizes(AgentIdentity::new("horizon:agent-b", Some(HOST_A))));
        assert!(!after_move.authorizes(member()));

        let other_host = stamp(HOST_B, &["horizon:agent-a"]);
        assert_eq!(
            sync_host_state_at(&path, "panel", false, &other_host)
                .expect_err("another live host must not rewrite the stamp")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(
            read_at(&path)
                .expect("read unchanged")
                .authorizes(AgentIdentity::new("horizon:agent-b", Some(HOST_A))),
            "the driver host's stamp survives a foreign sync"
        );

        let legacy = manifest_path_for_root(root.path(), "legacy");
        write_at(&legacy, &driver_manifest(None)).expect("write legacy manifest");
        assert_eq!(
            sync_host_state_at(&legacy, "legacy", true, &workspace)
                .expect_err("a manifest without a recorded host is never stamped")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );

        let missing = manifest_path_for_root(root.path(), "missing");
        assert_eq!(
            sync_host_state_at(&missing, "missing", true, &workspace)
                .expect_err("missing manifest must fail")
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn requested_panels_are_stamped_and_claimed_in_one_transaction() {
        let root = tempfile::tempdir().expect("isolated root");
        let path = manifest_path_for_root(root.path(), "panel");
        write_at(&path, &driver_manifest(Some(HOST_A))).expect("write manifest");
        let workspace = stamp(HOST_A, &["horizon:agent-a", "horizon:agent-b"]);

        assert_eq!(
            publish_requested_panel_at(
                &path,
                "panel",
                false,
                &workspace,
                AgentIdentity::new("horizon:agent-c", Some(HOST_A))
            )
            .expect_err("an owner outside the workspace is refused")
            .to_string(),
            OUTSIDE_WORKSPACE_MESSAGE
        );
        assert!(read_at(&path).expect("read").owner.is_none());

        publish_requested_panel_at(&path, "panel", false, &workspace, member()).expect("stamp and claim");
        let published = read_at(&path).expect("read published");
        assert!(published.hidden);
        assert_eq!(published.workspace.as_ref(), Some(&workspace));
        assert_eq!(
            published.owner.as_ref().map(|owner| owner.name.as_str()),
            Some("horizon:agent-a")
        );
        assert!(published.authorizes(member()));

        assert_eq!(
            publish_requested_panel_at(
                &path,
                "panel",
                true,
                &workspace,
                AgentIdentity::new("horizon:agent-b", Some(HOST_A))
            )
            .expect_err("a live owner blocks a second requester")
            .to_string(),
            "browser panel already has another live owner"
        );
        assert_eq!(
            publish_requested_panel_at(
                &path,
                "panel",
                true,
                &stamp(HOST_B, &["horizon:agent-a"]),
                AgentIdentity::new("horizon:agent-a", Some(HOST_B))
            )
            .expect_err("another host cannot publish this panel")
            .to_string(),
            OTHER_HOST_MESSAGE
        );
        assert_eq!(
            publish_requested_panel_at(
                &manifest_path_for_root(root.path(), "missing"),
                "missing",
                true,
                &workspace,
                member()
            )
            .expect_err("missing manifest")
            .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[test]
    fn legacy_manifests_deserialize_without_a_workspace_stamp() {
        let manifest: BrowserManifest = serde_json::from_str(
            r#"{
                "panel_local_id": "legacy",
                "browser_ws": "",
                "target_id": "",
                "url": "",
                "title": "",
                "user_active": false,
                "user_active_at": 0,
                "updated_at": 0
            }"#,
        )
        .expect("legacy manifest parses");
        assert!(manifest.workspace.is_none());
        assert!(!manifest.authorizes(member()));

        let stamped_without_host: BrowserManifest = serde_json::from_str(
            r#"{
                "panel_local_id": "old-stamp",
                "browser_ws": "",
                "target_id": "",
                "url": "",
                "title": "",
                "workspace": { "local_id": "ws-a", "actors": ["horizon:agent-a"] },
                "user_active": false,
                "user_active_at": 0,
                "updated_at": 0
            }"#,
        )
        .expect("host-less stamp parses");
        assert!(
            !stamped_without_host.permits(member()),
            "a stamp without a host instance never matches a live host"
        );
    }
}
