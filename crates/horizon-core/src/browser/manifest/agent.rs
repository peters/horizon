//! Safe agent-side ownership, steering, and action-queue helpers.

use std::path::Path;

use horizon_browser::{
    AgentAction, BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, BrowserControlAction,
    new_action_id,
};

use super::workspace::{OUTSIDE_WORKSPACE_MESSAGE, permit_actor};
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
pub fn claim(panel_local_id: &str, agent_name: &str, tty: Option<&str>) -> std::io::Result<()> {
    validate_actor(agent_name)?;
    if let Some(tty) = tty {
        validate_tty(tty)?;
    }
    let now = now_millis();
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = claim_locked(manifest, agent_name, tty, now);
    })?;
    outcome
}

fn claim_locked(manifest: &mut BrowserManifest, agent_name: &str, tty: Option<&str>, now: i64) -> std::io::Result<()> {
    permit_actor(manifest, agent_name)?;
    if try_claim_owner(manifest, agent_name, tty, now) {
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
pub fn heartbeat(panel_local_id: &str, agent_name: &str) -> std::io::Result<()> {
    validate_actor(agent_name)?;
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = heartbeat_locked(manifest, agent_name, now_millis());
    })?;
    outcome
}

fn heartbeat_locked(manifest: &mut BrowserManifest, agent_name: &str, now: i64) -> std::io::Result<()> {
    permit_actor(manifest, agent_name)?;
    if manifest.live_owner(now).is_some_and(|owner| owner.name == agent_name) {
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
pub fn request_handoff(panel_local_id: &str, agent_name: &str, reason: &str) -> std::io::Result<String> {
    validate_actor(agent_name)?;
    validate_reason(reason)?;
    let request_id = new_handoff_request_id();
    let mut outcome = Ok(());
    update(panel_local_id, |manifest| {
        outcome = request_handoff_locked(manifest, agent_name, &request_id, reason, now_millis());
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

/// Queue one validated backend-neutral action for the live owner.
///
/// The queue refuses input while the user is actively steering or a handoff
/// remains pending. This is the canonical auditable control path; direct raw
/// protocol clients are intentionally outside the contract.
///
/// # Errors
/// Returns `WouldBlock` while the user owns the wheel, `PermissionDenied` for
/// stale ownership, or another error for invalid input/I/O.
pub fn enqueue_action(panel_local_id: &str, agent_name: &str, action: BrowserControlAction) -> std::io::Result<String> {
    validate_actor(agent_name)?;
    action
        .validate()
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
    let action_id = new_action_id();
    let summary = BrowserAuditAction::from_control(&action);
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
        if !manifest.permits_actor(agent_name) {
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
        Ok(action_id)
    }
}

fn request_handoff_locked(
    manifest: &mut BrowserManifest,
    agent_name: &str,
    request_id: &str,
    reason: &str,
    now: i64,
) -> std::io::Result<()> {
    permit_actor(manifest, agent_name)?;
    if manifest.live_owner(now).is_some_and(|owner| owner.name == agent_name) {
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

pub(super) fn take_ready_actions(manifest: &mut BrowserManifest) -> (Vec<AgentAction>, Vec<AgentAction>) {
    let now = now_millis();
    if manifest.user_is_active(now) || manifest.handoff_pending().is_some() {
        return (Vec::new(), Vec::new());
    }
    let owner = manifest.live_owner(now).map(|owner| owner.name.clone());
    let mut ready = Vec::new();
    let mut rejected = Vec::new();
    for action in std::mem::take(&mut manifest.actions) {
        if owner.as_deref() == Some(action.actor.as_str()) {
            ready.push(action);
        } else {
            rejected.push(action);
        }
    }
    (ready, rejected)
}

pub(super) fn append_rejected_actions(panel_local_id: &str, actions: Vec<AgentAction>) -> std::io::Result<()> {
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

fn set_owner(manifest: &mut BrowserManifest, agent_name: &str, tty: Option<&str>, now: i64) {
    manifest.owner = Some(ManifestOwner {
        name: agent_name.to_string(),
        tty: tty.map(str::to_string),
        updated_at: now,
    });
    manifest.updated_at = now;
}

fn try_claim_owner(manifest: &mut BrowserManifest, agent_name: &str, tty: Option<&str>, now: i64) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manifest::{BrowserManifest, write_at};

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
        let member = "horizon:agent-a";
        let outsider = "horizon:agent-b";
        let mut manifest = BrowserManifest {
            panel_local_id: "panel".to_string(),
            workspace: Some(super::super::ManifestWorkspace::new("ws-a", vec![member.to_string()])),
            ..BrowserManifest::default()
        };

        let denied = claim_locked(&mut manifest, outsider, None, now).expect_err("outsider cannot claim");
        assert_eq!(denied.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(denied.to_string(), OUTSIDE_WORKSPACE_MESSAGE);
        assert!(manifest.owner.is_none(), "a refused claim leaves the panel unowned");
        claim_locked(&mut manifest, member, None, now).expect("member claims");
        heartbeat_locked(&mut manifest, member, now + 1).expect("member heartbeats");
        request_handoff_locked(&mut manifest, member, "request-1", "sign in", now + 2).expect("member hands off");
        manifest.handoff = None;

        // The host moved the panel away while the member still holds the lease.
        manifest.workspace = Some(super::super::ManifestWorkspace::new("ws-b", vec![outsider.to_string()]));
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
            "browser-cli-test",
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
        assert!(released.handoff.is_none());
        assert!(!release_at(&path, "panel", "agent-a").unwrap());
    }
}
