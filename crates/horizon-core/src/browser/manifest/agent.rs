//! Safe agent-side ownership, steering, and action-queue helpers.

#[cfg(test)]
use std::path::Path;

use horizon_browser::{
    AgentAction, BrowserAuditAction, BrowserAuditActor, BrowserAuditEntry, BrowserAuditStatus, BrowserControlAction,
    new_action_id,
};

#[cfg(test)]
use super::update_at;
use super::{BrowserManifest, ManifestHandoff, ManifestOwner, new_handoff_request_id, now_millis, update};

const MAX_PENDING_ACTIONS: usize = 128;
const MAX_ACTOR_BYTES: usize = 128;
const MAX_HANDOFF_REASON_BYTES: usize = 2 * 1024;

/// Claim or refresh ownership of a live panel for an external agent.
///
/// # Errors
/// Returns an error for invalid identity, a missing live manifest, or a
/// filesystem failure.
pub fn claim(panel_local_id: &str, agent_name: &str, tty: Option<&str>) -> std::io::Result<()> {
    validate_actor(agent_name)?;
    if let Some(tty) = tty {
        validate_tty(tty)?;
    }
    let now = now_millis();
    let mut claimed = false;
    update(panel_local_id, |manifest| {
        claimed = try_claim_owner(manifest, agent_name, tty, now);
    })?;
    if claimed {
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
/// Returns `PermissionDenied` when this agent is not the current owner.
pub fn heartbeat(panel_local_id: &str, agent_name: &str) -> std::io::Result<()> {
    validate_actor(agent_name)?;
    let mut matched = false;
    update(panel_local_id, |manifest| {
        let now = now_millis();
        if manifest.live_owner(now).is_some_and(|owner| owner.name == agent_name) {
            if let Some(owner) = manifest.owner.as_mut() {
                owner.updated_at = now;
            }
            manifest.updated_at = now;
            matched = true;
        }
    })?;
    if matched {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agent does not own this browser panel",
        ))
    }
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
    let mut authorized = false;
    update(panel_local_id, |manifest| {
        if manifest
            .live_owner(now_millis())
            .is_some_and(|owner| owner.name == agent_name)
        {
            manifest.handoff = Some(ManifestHandoff {
                request_id: request_id.clone(),
                reason: reason.to_string(),
                requested_at: now_millis(),
                done: false,
            });
            manifest.updated_at = now_millis();
            authorized = true;
        }
    })?;
    if !authorized {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agent does not have a live ownership claim",
        ));
    }
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
        if manifest.live_owner(now).is_none_or(|owner| owner.name != agent_name) {
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

fn validate_actor(actor: &str) -> std::io::Result<()> {
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
}
