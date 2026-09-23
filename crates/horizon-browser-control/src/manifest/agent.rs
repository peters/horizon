//! Safe agent-side ownership, steering, and action-queue helpers.

use std::path::Path;

use horizon_browser::{
    AgentAction, BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, BrowserControlAction,
    new_action_id,
};

use super::workspace::{AgentIdentity, OUTSIDE_WORKSPACE_MESSAGE, permit};
use super::{BrowserManifest, ManifestHandoff, ManifestOwner, new_handoff_request_id, now_millis, update, update_at};

const MAX_PENDING_ACTIONS: usize = 128;
const MAX_ACTOR_BYTES: usize = 128;
const MAX_HANDOFF_REASON_BYTES: usize = 2 * 1024;

/// Claim or refresh ownership of a live panel for an external agent.
///
/// Workspace membership is checked inside the same locked transaction as
/// the claim, so a host re-stamp during a panel move cannot race past it.
///
/// # Errors
/// Returns an error for invalid identity, a missing live manifest, an
/// identity outside the panel's workspace, or a filesystem failure.
pub fn claim(panel_local_id: &str, identity: AgentIdentity<'_>, tty: Option<&str>) -> std::io::Result<()> {
    validate_actor(identity.actor)?;
    if let Some(tty) = tty {
        validate_tty(tty)?;
    }
    let now = now_millis();
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = claim_locked(manifest, identity, tty, now);
    })?;
    outcome
}

fn claim_locked(
    manifest: &mut BrowserManifest,
    identity: AgentIdentity<'_>,
    tty: Option<&str>,
    now: i64,
) -> std::io::Result<()> {
    permit(manifest, identity)?;
    if try_claim_owner(manifest, identity.actor, tty, now) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "browser panel already has another live owner",
        ))
    }
}

/// Refresh an existing ownership claim without taking it from another agent.
///
/// # Errors
/// Returns `PermissionDenied` when this agent is not the current owner or is
/// outside the panel's workspace.
pub fn heartbeat(panel_local_id: &str, identity: AgentIdentity<'_>) -> std::io::Result<()> {
    validate_actor(identity.actor)?;
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = heartbeat_locked(manifest, identity, now_millis());
    })?;
    outcome
}

fn heartbeat_locked(manifest: &mut BrowserManifest, identity: AgentIdentity<'_>, now: i64) -> std::io::Result<()> {
    permit(manifest, identity)?;
    if manifest
        .live_owner(now)
        .is_some_and(|owner| owner.name == identity.actor)
    {
        if let Some(owner) = manifest.owner.as_mut() {
            owner.updated_at = now;
        }
        manifest.updated_at = now;
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agent does not own this browser panel",
        ))
    }
}

/// Renew only the recorded owner of an existing handoff after a wait timeout.
/// A lease may expire while the model reads the timeout; the owner and request
/// must still match under the manifest lock. Never reset the user's Done flag.
///
/// # Errors
/// Returns `PermissionDenied` when ownership, request, or workspace changed.
pub fn resume_handoff(panel_local_id: &str, identity: AgentIdentity<'_>, request_id: &str) -> std::io::Result<()> {
    validate_actor(identity.actor)?;
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = permit(manifest, identity).and_then(|()| {
            if manifest.owner.as_ref().is_none_or(|owner| owner.name != identity.actor)
                || manifest
                    .handoff
                    .as_ref()
                    .is_none_or(|handoff| handoff.request_id != request_id)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "handoff owner or request changed; inspect browser_panel before continuing",
                ));
            }
            let now = now_millis();
            if let Some(owner) = manifest.owner.as_mut() {
                owner.updated_at = now;
            }
            manifest.updated_at = now;
            Ok(())
        });
    })?;
    outcome
}

/// Release a panel only when `agent_name` is still its recorded owner.
///
/// This is the clean-shutdown counterpart to the heartbeat TTL. A mismatched
/// owner is left untouched and returns `false`, so a stale process cannot
/// release a claim that another agent has since acquired.
///
/// # Errors
/// Returns an error for invalid identity, a missing live manifest, or a
/// filesystem failure.
pub fn release(panel_local_id: &str, agent_name: &str) -> std::io::Result<bool> {
    release_at(
        &super::default_manifest_path(panel_local_id),
        panel_local_id,
        agent_name,
    )
}

fn release_at(path: &Path, panel_local_id: &str, agent_name: &str) -> std::io::Result<bool> {
    validate_actor(agent_name)?;
    let mut released = false;
    update_at(path, panel_local_id, |manifest| {
        if manifest.owner.as_ref().is_some_and(|owner| owner.name == agent_name) {
            manifest.owner = None;
            manifest.handoff = None;
            manifest.updated_at = now_millis();
            released = true;
        }
    })?;
    Ok(released)
}

/// Ask the user to steer the panel, pausing queued agent actions until the
/// exact request is handed back.
///
/// # Errors
/// Returns an error for an invalid request, stale ownership, or I/O failure.
pub fn request_handoff(panel_local_id: &str, identity: AgentIdentity<'_>, reason: &str) -> std::io::Result<String> {
    let agent_name = identity.actor;
    validate_actor(agent_name)?;
    validate_reason(reason)?;
    let request_id = new_handoff_request_id();
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = request_handoff_locked(manifest, identity, &request_id, reason, now_millis());
    })?;
    outcome?;
    if let Err(error) = super::audit::append(
        &BrowserAuditEntry::new(
            request_id.clone(),
            BrowserAuditActor::Agent {
                name: agent_name.to_string(),
            },
            BrowserAuditStatus::Dispatched,
            BrowserAuditAction::HandoffRequested,
        ),
        panel_local_id,
    ) {
        tracing::warn!(target: "browser", "failed to append handoff audit: {error}");
    }
    Ok(request_id)
}

/// A `set_files` action is queued with private staged copies of its files:
/// the paths are resolved and confirmed under the attachment roots of this
/// process, the audit summary keeps those resolved paths, and the engine
/// receives copies that the original pathnames can no longer influence.
/// The copies stay for the panel (bounded by age, count and size) because
/// the page reads them lazily. The rebuilt action is validated again
/// because resolution can change a pathname. Every other action passes
/// through.
fn authorize_attachments(
    action: BrowserControlAction,
    panel_local_id: &str,
    action_id: &str,
    remote: bool,
) -> std::io::Result<(BrowserControlAction, BrowserAuditAction, Option<StagedAttachments>)> {
    let BrowserControlAction::SetFiles { target, paths, .. } = action else {
        let summary = BrowserAuditAction::from_control(&action);
        return Ok((action, summary, None));
    };
    // The policy error travels inside the I/O error so a caller can recover
    // its typed classification from this authoritative pass too.
    let refused = |error: crate::AttachmentPolicyError| std::io::Error::new(error.io_kind(), error);
    let authorized = crate::AttachmentPolicy::from_environment()
        .authorize(&paths)
        .map_err(refused)?;
    if remote {
        crate::attachments::check_remote_budget(&authorized).map_err(refused)?;
    }
    let sources = authorized
        .iter()
        .map(|file| file.path().to_path_buf())
        .collect::<Vec<_>>();
    let summary = BrowserAuditAction::from_control(&BrowserControlAction::SetFiles {
        target: target.clone(),
        paths: sources.clone(),
        sources: Vec::new(),
    });
    let reserved_bytes =
        crate::attachments::check_panel_budget(&authorized, crate::attachments::MAX_RETAINED_ATTACHMENT_BYTES)
            .map_err(refused)?;
    let attachments_dir = crate::BrowserRuntimePaths::resolve().browser_attachments_dir();
    // One panel stages one action at a time, so pruning for the reserved
    // size and copying happen under the same lock and concurrent enqueues
    // cannot both fit their files into the same budget. The lock is a
    // sibling of the panel's staging directory, not the manifest lock, so
    // a long copy never blocks the engine.
    create_staging_directory(&attachments_dir)?;
    let lock = super::ManifestLock::acquire_with_timeout(
        &attachments_dir.join(crate::paths::safe_local_id(panel_local_id)),
        STAGING_LOCK_WAIT,
    )?;
    let manifests = crate::BrowserRuntimePaths::resolve().browsers_manifest_dir();
    crate::attachments::prune_attachments(
        &attachments_dir,
        panel_local_id,
        |panel| {
            let mut manifest = std::ffi::OsString::from(panel);
            manifest.push(".json");
            // A liveness check that cannot be answered keeps the staging.
            manifests.join(manifest).try_exists().unwrap_or(true)
        },
        crate::attachments::ATTACHMENT_RETENTION,
        crate::attachments::MAX_RETAINED_ATTACHMENT_ACTIONS,
        crate::attachments::MAX_RETAINED_ATTACHMENT_BYTES,
        reserved_bytes,
    )
    .map_err(refused)?;
    let staged = StagedAttachments {
        attachments_dir: attachments_dir.clone(),
        panel_local_id: panel_local_id.to_string(),
        action_id: action_id.to_string(),
        keep: false,
        _lock: lock,
    };
    let paths = crate::attachments::stage_attachments(&attachments_dir, panel_local_id, action_id, &authorized)
        .map_err(refused)?;
    // Engines read the staged copies; every audit record shows the sources.
    let action = BrowserControlAction::SetFiles { target, paths, sources };
    action
        .validate()
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
    Ok((action, summary, Some(staged)))
}

/// The same eligibility the locked enqueue applies, read without the lock:
/// a cheap refusal for callers whose action would be rejected anyway.
fn check_enqueue_eligibility(
    panel_local_id: &str,
    identity: AgentIdentity<'_>,
    agent_name: &str,
) -> std::io::Result<bool> {
    let manifest = super::read(panel_local_id)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "browser panel is not live"))?;
    let now = now_millis();
    let refusal = if manifest.remote_target.is_some() && !manifest.remote_file_upload {
        Some((
            std::io::ErrorKind::Unsupported,
            "file attachment is unavailable for remote device sessions",
        ))
    } else if !manifest.permits(identity) {
        Some((std::io::ErrorKind::PermissionDenied, OUTSIDE_WORKSPACE_MESSAGE))
    } else if manifest.live_owner(now).is_none_or(|owner| owner.name != agent_name) {
        Some((
            std::io::ErrorKind::PermissionDenied,
            "agent does not have a live ownership claim",
        ))
    } else if manifest.user_is_active(now) || manifest.handoff_pending().is_some() {
        Some((std::io::ErrorKind::WouldBlock, "user is steering this browser panel"))
    } else if manifest.actions.len() >= MAX_PENDING_ACTIONS {
        Some((std::io::ErrorKind::WouldBlock, "browser action queue is full"))
    } else {
        None
    };
    refusal.map_or(Ok(manifest.remote_target.is_some()), |(kind, message)| {
        Err(std::io::Error::new(kind, message))
    })
}

/// Longest a `set_files` enqueue waits for the panel's staging lock while
/// another attachment on the same panel is being copied.
const STAGING_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Staged copies that are removed unless the action reached the queue; the
/// panel's staging lock is held until the action is queued or released.
struct StagedAttachments {
    attachments_dir: std::path::PathBuf,
    panel_local_id: String,
    action_id: String,
    keep: bool,
    _lock: super::ManifestLock,
}

impl Drop for StagedAttachments {
    fn drop(&mut self) {
        if !self.keep {
            crate::attachments::release_attachments(&self.attachments_dir, &self.panel_local_id, &self.action_id);
        }
    }
}

/// Queue one validated backend-neutral action for the live owner.
///
/// The queue refuses input while the user is actively steering or a handoff
/// remains pending. This is the canonical auditable control path; direct raw
/// protocol clients are intentionally outside the contract.
///
/// # Errors
/// Returns `WouldBlock` while the user owns the wheel, `PermissionDenied` for
/// stale ownership, or another error for invalid input/I/O.
pub fn enqueue_action(
    panel_local_id: &str,
    identity: AgentIdentity<'_>,
    action: BrowserControlAction,
) -> std::io::Result<String> {
    let agent_name = identity.actor;
    validate_actor(agent_name)?;
    action
        .validate()
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
    let action_id = new_action_id();
    let remote = if matches!(action, BrowserControlAction::SetFiles { .. }) {
        // Refuse ineligible callers before any filesystem work, and carry
        // the target's transfer limits into the authoritative authorization.
        check_enqueue_eligibility(panel_local_id, identity, agent_name)?
    } else {
        false
    };
    let (action, summary, mut staged) = authorize_attachments(action, panel_local_id, &action_id, remote)?;
    let request = AgentAction {
        action_id: action_id.clone(),
        actor: agent_name.to_string(),
        requested_at_millis: now_millis(),
        action,
    };
    let mut failure = None;
    let mut audit_failure = None;
    update(panel_local_id, |manifest| {
        let now = now_millis();
        if !manifest.permits(identity) {
            failure = Some((std::io::ErrorKind::PermissionDenied, OUTSIDE_WORKSPACE_MESSAGE));
        } else if manifest.live_owner(now).is_none_or(|owner| owner.name != agent_name) {
            failure = Some((
                std::io::ErrorKind::PermissionDenied,
                "agent does not have a live ownership claim",
            ));
        } else if manifest.user_is_active(now) || manifest.handoff_pending().is_some() {
            failure = Some((std::io::ErrorKind::WouldBlock, "user is steering this browser panel"));
        } else if manifest.actions.len() >= MAX_PENDING_ACTIONS {
            failure = Some((std::io::ErrorKind::WouldBlock, "browser action queue is full"));
        } else if let Err(error) = super::audit::append(
            &BrowserAuditEntry::new(
                action_id.clone(),
                BrowserAuditActor::Agent {
                    name: agent_name.to_string(),
                },
                BrowserAuditStatus::Queued,
                summary.clone(),
            ),
            panel_local_id,
        ) {
            audit_failure = Some(error);
        } else {
            manifest.actions.push(request);
            manifest.updated_at = now;
        }
    })?;

    if let Some(error) = audit_failure {
        return Err(error);
    }
    if let Some((kind, message)) = failure {
        if let Err(error) = super::audit::append(
            &BrowserAuditEntry::new(
                action_id,
                BrowserAuditActor::Agent {
                    name: agent_name.to_string(),
                },
                BrowserAuditStatus::Rejected,
                summary,
            ),
            panel_local_id,
        ) {
            tracing::warn!(target: "browser", "failed to append rejected-action audit: {error}");
        }
        Err(std::io::Error::new(kind, message))
    } else {
        if let Some(staged) = staged.as_mut() {
            staged.keep = true;
        }
        Ok(action_id)
    }
}

fn request_handoff_locked(
    manifest: &mut BrowserManifest,
    identity: AgentIdentity<'_>,
    request_id: &str,
    reason: &str,
    now: i64,
) -> std::io::Result<()> {
    permit(manifest, identity)?;
    if manifest
        .live_owner(now)
        .is_some_and(|owner| owner.name == identity.actor)
    {
        manifest.handoff = Some(ManifestHandoff {
            request_id: request_id.to_string(),
            reason: reason.to_string(),
            requested_at: now,
            done: false,
        });
        manifest.updated_at = now;
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agent does not have a live ownership claim",
        ))
    }
}

/// Promote queued actions for the driver. An action is ready only if its
/// actor still holds the live lease and the current stamp still permits it;
/// an action queued before the host moved the panel is rejected here so the
/// move revokes browser-side effects, not only result disclosure.
pub(super) fn take_ready_actions(manifest: &mut BrowserManifest) -> (Vec<AgentAction>, Vec<AgentAction>) {
    let now = now_millis();
    if manifest.user_is_active(now) || manifest.handoff_pending().is_some() {
        return (Vec::new(), Vec::new());
    }
    let owner = manifest.live_owner(now).map(|owner| owner.name.clone());
    let driver_host = manifest.host.clone();
    let mut ready = Vec::new();
    let mut rejected = Vec::new();
    for action in std::mem::take(&mut manifest.actions) {
        let still_permitted = manifest.permits(AgentIdentity::new(&action.actor, driver_host.as_deref()));
        if still_permitted && owner.as_deref() == Some(action.actor.as_str()) {
            ready.push(action);
        } else {
            rejected.push(action);
        }
    }
    (ready, rejected)
}

pub(super) fn append_rejected_actions(panel_local_id: &str, actions: Vec<AgentAction>) -> std::io::Result<()> {
    release_rejected_attachments(
        &crate::BrowserRuntimePaths::resolve().browser_attachments_dir(),
        panel_local_id,
        &actions,
    );
    for request in actions {
        super::audit::append(
            &BrowserAuditEntry::new(
                request.action_id,
                BrowserAuditActor::Agent { name: request.actor },
                BrowserAuditStatus::Rejected,
                BrowserAuditAction::from_control(&request.action),
            ),
            panel_local_id,
        )?;
    }
    Ok(())
}

fn release_rejected_attachments(attachments_dir: &Path, panel_local_id: &str, actions: &[AgentAction]) {
    // All of these actions left the queue without reaching a driver. Release
    // every staging directory even if recording an audit entry later fails.
    for request in actions {
        if matches!(request.action, BrowserControlAction::SetFiles { .. }) {
            crate::attachments::release_attachments(attachments_dir, panel_local_id, &request.action_id);
        }
    }
}

fn set_owner(manifest: &mut BrowserManifest, agent_name: &str, tty: Option<&str>, now: i64) {
    manifest.ownership_established = Some(true);
    manifest.owner = Some(ManifestOwner {
        name: agent_name.to_string(),
        tty: tty.map(str::to_string),
        updated_at: now,
    });
    manifest.updated_at = now;
}

pub(super) fn try_claim_owner(manifest: &mut BrowserManifest, agent_name: &str, tty: Option<&str>, now: i64) -> bool {
    if manifest.live_owner(now).is_some_and(|owner| owner.name != agent_name) {
        return false;
    }
    set_owner(manifest, agent_name, tty, now);
    true
}

fn validate_tty(tty: &str) -> std::io::Result<()> {
    if tty.len() > 512 || tty.chars().any(char::is_control) {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "agent tty must be a short printable value",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_actor(actor: &str) -> std::io::Result<()> {
    if actor.trim().is_empty() || actor.len() > MAX_ACTOR_BYTES || actor.chars().any(char::is_control) {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "agent name must be a short printable value",
        ))
    } else {
        Ok(())
    }
}

fn validate_reason(reason: &str) -> std::io::Result<()> {
    if reason.trim().is_empty() || reason.len() > MAX_HANDOFF_REASON_BYTES || reason.chars().any(char::is_control) {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "handoff reason must be a short printable value",
        ))
    } else {
        Ok(())
    }
}

/// Keep the OS error intact: the MCP tells a coordination refusal from an
/// operating system failure by its OS error code (#847).
fn create_staging_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory).inspect_err(|error| {
        tracing::warn!(target: "browser", "could not prepare the attachment staging directory: {error}");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{BrowserManifest, write_at};

    fn manifest(path: &Path) {
        write_at(
            path,
            &BrowserManifest {
                panel_local_id: "panel".to_string(),
                ..BrowserManifest::default()
            },
        )
        .unwrap();
    }

    #[test]
    fn ownership_rejection_releases_only_the_rejected_attachment_staging() {
        let root = tempfile::tempdir().expect("root");
        let source = root.path().join("notes.txt");
        std::fs::write(&source, "fixture").expect("source");
        let authorized = crate::AttachmentPolicy::new([root.path().to_path_buf()])
            .authorize(&[source])
            .expect("authorize");
        let staging = root.path().join("staging");
        let mut manifest = BrowserManifest {
            panel_local_id: "panel".into(),
            owner: Some(ManifestOwner {
                name: "current".into(),
                tty: None,
                updated_at: now_millis(),
            }),
            ..BrowserManifest::default()
        };
        for (id, actor) in [("keep", "current"), ("reject", "previous")] {
            let paths = crate::attachments::stage_attachments(&staging, "panel", id, &authorized).expect("stage");
            manifest.actions.push(AgentAction {
                action_id: id.into(),
                actor: actor.into(),
                requested_at_millis: now_millis(),
                action: BrowserControlAction::SetFiles {
                    target: horizon_browser::BrowserTarget::Selector {
                        selector: "#file".into(),
                    },
                    paths,
                    sources: Vec::new(),
                },
            });
        }
        let kept_path = match &manifest.actions[0].action {
            BrowserControlAction::SetFiles { paths, .. } => paths[0].clone(),
            _ => unreachable!(),
        };
        let rejected_path = match &manifest.actions[1].action {
            BrowserControlAction::SetFiles { paths, .. } => paths[0].clone(),
            _ => unreachable!(),
        };
        let (ready, rejected) = take_ready_actions(&mut manifest);
        assert_eq!(ready.len(), 1);
        assert_eq!(rejected.len(), 1);
        release_rejected_attachments(&staging, "panel", &rejected);
        assert!(kept_path.exists());
        assert!(!rejected_path.exists());
    }

    #[test]
    fn ready_actions_require_the_live_owner_and_no_user_steering() {
        let now = now_millis();
        let mut manifest = BrowserManifest {
            panel_local_id: "panel".to_string(),
            owner: Some(ManifestOwner {
                name: "agent-a".to_string(),
                tty: None,
                updated_at: now,
            }),
            actions: vec![
                AgentAction {
                    action_id: "a".to_string(),
                    actor: "agent-a".to_string(),
                    requested_at_millis: now,
                    action: BrowserControlAction::Reload,
                },
                AgentAction {
                    action_id: "b".to_string(),
                    actor: "agent-b".to_string(),
                    requested_at_millis: now,
                    action: BrowserControlAction::Back,
                },
            ],
            ..BrowserManifest::default()
        };

        let (ready, rejected) = take_ready_actions(&mut manifest);

        assert_eq!(
            ready.iter().map(|action| action.action_id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        assert_eq!(
            rejected
                .iter()
                .map(|action| action.action_id.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        manifest.user_active = true;
        manifest.user_active_at = now_millis();
        manifest.actions.push(ready[0].clone());
        assert!(take_ready_actions(&mut manifest).0.is_empty());
    }

    #[test]
    fn queued_actions_are_rejected_once_the_stamp_no_longer_permits_their_actor() {
        let now = now_millis();
        let queued = || AgentAction {
            action_id: "queued".to_string(),
            actor: "horizon:agent-a".to_string(),
            requested_at_millis: now,
            action: BrowserControlAction::Reload,
        };
        let mut manifest = BrowserManifest {
            panel_local_id: "panel".to_string(),
            host: Some("host-a".to_string()),
            workspace: Some(super::super::ManifestWorkspace::new(
                "host-a",
                "ws-a",
                vec!["horizon:agent-a".to_string()],
            )),
            owner: Some(ManifestOwner {
                name: "horizon:agent-a".to_string(),
                tty: None,
                updated_at: now,
            }),
            actions: vec![queued()],
            ..BrowserManifest::default()
        };

        let (ready, rejected) = take_ready_actions(&mut manifest);
        assert_eq!(ready.len(), 1);
        assert!(rejected.is_empty());

        // The host moved the panel after the action was queued.
        manifest.workspace = Some(super::super::ManifestWorkspace::new(
            "host-a",
            "ws-b",
            vec!["horizon:agent-b".to_string()],
        ));
        manifest.actions.push(queued());
        let (ready, rejected) = take_ready_actions(&mut manifest);
        assert!(ready.is_empty(), "a moved panel must not execute the stale action");
        assert_eq!(rejected.len(), 1);

        // A stamp left by a previous host is equally stale.
        manifest.workspace = Some(super::super::ManifestWorkspace::new(
            "host-b",
            "ws-a",
            vec!["horizon:agent-a".to_string()],
        ));
        manifest.actions.push(queued());
        assert!(take_ready_actions(&mut manifest).0.is_empty());
    }

    #[test]
    fn owner_updates_use_the_locked_manifest_transaction() {
        let root = std::env::temp_dir().join(format!("horizon-agent-{}", std::process::id()));
        let path = root.join("runtime/browsers/panel.json");
        manifest(&path);

        let now = now_millis();
        update_at(&path, "panel", |manifest| {
            set_owner(manifest, "agent", Some("pts/1"), now);
        })
        .unwrap();

        let updated = super::super::read_at(&path).unwrap();
        assert_eq!(updated.owner.as_ref().map(|owner| owner.name.as_str()), Some("agent"));
        assert_eq!(
            updated.owner.as_ref().and_then(|owner| owner.tty.as_deref()),
            Some("pts/1")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn claim_never_steals_a_fresh_owner() {
        let now = now_millis();
        let mut manifest = BrowserManifest {
            owner: Some(ManifestOwner {
                name: "agent-a".to_string(),
                tty: None,
                updated_at: now,
            }),
            ..BrowserManifest::default()
        };

        assert!(!try_claim_owner(&mut manifest, "agent-b", None, now));
        assert_eq!(
            manifest.owner.as_ref().map(|owner| owner.name.as_str()),
            Some("agent-a")
        );
        assert!(try_claim_owner(&mut manifest, "agent-a", Some("pts/2"), now));
        assert_eq!(
            manifest.owner.as_ref().and_then(|owner| owner.tty.as_deref()),
            Some("pts/2")
        );
        assert!(try_claim_owner(
            &mut manifest,
            "agent-b",
            None,
            now + super::super::OWNER_TTL_MILLIS + 1,
        ));
        assert_eq!(
            manifest.owner.as_ref().map(|owner| owner.name.as_str()),
            Some("agent-b")
        );
    }

    #[test]
    fn locked_transactions_refuse_identities_outside_the_workspace() {
        let now = now_millis();
        let member = AgentIdentity::new("horizon:agent-a", Some("host-a"));
        let outsider = AgentIdentity::new("horizon:agent-b", Some("host-a"));
        let twin = AgentIdentity::new("horizon:agent-a", Some("host-b"));
        let unbound = AgentIdentity::new("horizon:agent-a", None);
        let mut manifest = BrowserManifest {
            panel_local_id: "panel".to_string(),
            host: Some("host-a".to_string()),
            workspace: Some(super::super::ManifestWorkspace::new(
                "host-a",
                "ws-a",
                vec![member.actor.to_string()],
            )),
            ..BrowserManifest::default()
        };

        for refused in [outsider, twin, unbound] {
            let denied = claim_locked(&mut manifest, refused, None, now).expect_err("refused identity cannot claim");
            assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
            assert_eq!(denied.to_string(), OUTSIDE_WORKSPACE_MESSAGE);
        }
        assert!(manifest.owner.is_none(), "a refused claim leaves the panel unowned");
        claim_locked(&mut manifest, member, None, now).expect("member claims");
        heartbeat_locked(&mut manifest, member, now + 1).expect("member heartbeats");
        request_handoff_locked(&mut manifest, member, "request-1", "sign in", now + 2).expect("member hands off");
        manifest.handoff = None;

        // The host moved the panel away while the member still holds the lease.
        manifest.workspace = Some(super::super::ManifestWorkspace::new(
            "host-a",
            "ws-b",
            vec![outsider.actor.to_string()],
        ));
        assert_eq!(
            heartbeat_locked(&mut manifest, member, now + 3)
                .expect_err("moved panel refuses the old member")
                .to_string(),
            OUTSIDE_WORKSPACE_MESSAGE
        );
        assert_eq!(
            request_handoff_locked(&mut manifest, member, "request-2", "sign in", now + 3)
                .expect_err("moved panel refuses handoff")
                .to_string(),
            OUTSIDE_WORKSPACE_MESSAGE
        );
        assert!(manifest.handoff.is_none());
        assert_eq!(
            claim_locked(&mut manifest, outsider, None, now + 3)
                .expect_err("fresh lease still blocks the new member")
                .to_string(),
            "browser panel already has another live owner"
        );
        claim_locked(&mut manifest, outsider, None, now + super::super::OWNER_TTL_MILLIS + 4)
            .expect("new member claims after the lease expires");

        manifest.workspace = None;
        assert_eq!(
            heartbeat_locked(&mut manifest, outsider, now + super::super::OWNER_TTL_MILLIS + 5)
                .expect_err("an unstamped manifest refuses every Horizon agent")
                .to_string(),
            OUTSIDE_WORKSPACE_MESSAGE
        );
        claim_locked(
            &mut manifest,
            AgentIdentity::new("browser-cli-test", None),
            None,
            now + 2 * super::super::OWNER_TTL_MILLIS + 6,
        )
        .expect("unscoped identities keep the unscoped behavior");
    }

    #[test]
    fn release_clears_only_the_matching_owner_and_its_handoff() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("panel.json");
        write_at(
            &path,
            &BrowserManifest {
                panel_local_id: "panel".to_string(),
                ownership_established: Some(true),
                owner: Some(ManifestOwner {
                    name: "agent-a".to_string(),
                    tty: None,
                    updated_at: now_millis(),
                }),
                handoff: Some(ManifestHandoff {
                    request_id: "request".to_string(),
                    reason: "user steering".to_string(),
                    requested_at: now_millis(),
                    done: false,
                }),
                ..BrowserManifest::default()
            },
        )
        .unwrap();

        assert!(!release_at(&path, "panel", "agent-b").unwrap());
        let unchanged = super::super::read_at(&path).unwrap();
        assert_eq!(
            unchanged.owner.as_ref().map(|owner| owner.name.as_str()),
            Some("agent-a")
        );
        assert!(unchanged.handoff.is_some());

        assert!(release_at(&path, "panel", "agent-a").unwrap());
        let released = super::super::read_at(&path).unwrap();
        assert!(released.owner.is_none());
        assert_eq!(released.ownership_established, Some(true));
        assert!(released.handoff.is_none());
        assert!(!release_at(&path, "panel", "agent-a").unwrap());
    }

    #[test]
    fn staging_directory_failures_keep_their_os_error_code() {
        let root = tempfile::tempdir().unwrap();
        let blocker = root.path().join("attachments");
        std::fs::write(&blocker, b"not a directory").unwrap();

        let error = create_staging_directory(&blocker.join("panel")).unwrap_err();

        assert!(
            error.raw_os_error().is_some(),
            "a filesystem failure must stay distinguishable from a coordination refusal: {error:?}"
        );
    }
}
