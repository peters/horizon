//! Relaunching a cloud's recorded sessions after an image replacement reset its container.
use super::{Deployment, Error, Event, OperationId, Result, Session, Store, WorkerContract, activity};

pub(super) const RELAUNCH_STATUS: &str = "horizon-relaunch-status=";

/// After a replacement reset the worker's container, relaunches each recorded session's
/// process in its existing worktree and clears the request. A relaunch is idempotent per
/// replacement, so a reconnect retries safely. Images without the relaunch contract
/// report their sessions lost, as Stop and Resume do. A session of an agent that the
/// profile no longer has, after a rebuild removed it, is reported once and not
/// relaunched: an image that checks the profile's capabilities refuses to start that
/// agent. Its record stays, marked as reported so later reconnects stay quiet, and a
/// later rebuild that adds the agent again relaunches it and clears the mark.
/// # Errors
/// Keeps the request when a relaunch cannot be confirmed.
pub(in crate::cloud_runtime::deployment) fn relaunch_sessions(
    store: &Store,
    state: &mut Deployment,
    contract: &WorkerContract,
    emit: &dyn Fn(Event),
    mut run: impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    let Some(operation) = state.session_restart else {
        return Ok(());
    };
    if contract.session_restart && !state.sessions.is_empty() {
        emit(activity("Relaunching sessions in their existing worktrees"));
    }
    for session in &mut state.sessions {
        if contract.capabilities && !state.profile.capabilities.permits_agent(&session.agent) {
            if !session.removed_agent_reported {
                emit(Event::Output(format!(
                    "Session {} ({}): this cloud's profile no longer has agent {}, so its process was not relaunched",
                    session.panel_id, session.agent, session.agent
                )));
                session.removed_agent_reported = true;
            }
            continue;
        }
        session.removed_agent_reported = false;
        let relaunched = contract.session_restart
            && match relaunch_status(&run(&relaunch_command(operation, session, &state.revision)?)?) {
                Some(0 | 5) => true,
                Some(3 | 4) => false,
                _ => return Err(Error::Invalid("A session could not be relaunched; reconnect to retry")),
            };
        if !relaunched {
            emit(Event::Output(format!(
                "Session {} ({}) lost its process in the container reset and was not relaunched",
                session.panel_id, session.agent
            )));
        }
    }
    state.session_restart = None;
    store.save(state)
}

fn relaunch_command(operation: OperationId, session: &Session, revision: &str) -> Result<String> {
    if !horizon_cloud::valid_id(&session.panel_id)
        || !matches!(session.agent.as_str(), "codex" | "claude" | "grok" | "shell")
        || !crate::cloud_runtime::repository::is_commit_id(revision)
    {
        return Err(Error::Invalid("Invalid remote session identity"));
    }
    Ok(format!(
        "horizon-worker-session --relaunch {operation} {} {} {revision}; printf '\\n{RELAUNCH_STATUS}%s\\n' \"$?\"",
        session.panel_id, session.agent
    ))
}

fn relaunch_status(output: &str) -> Option<i32> {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(RELAUNCH_STATUS))
        .and_then(|status| status.parse().ok())
}
