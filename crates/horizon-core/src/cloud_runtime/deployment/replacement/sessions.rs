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
    let mut marked = false;
    let mut failure = None;
    for session in &mut state.sessions {
        if contract.capabilities && !state.profile.capabilities.permits_agent(&session.agent) {
            if !session.removed_agent_reported {
                emit(Event::Output(format!(
                    "Session {} ({}): this cloud's profile no longer has agent {}, so its process was not relaunched",
                    session.panel_id, session.agent, session.agent
                )));
                session.removed_agent_reported = true;
                marked = true;
            }
            continue;
        }
        session.removed_agent_reported = false;
        match relaunch(operation, session, &state.revision, contract, &mut run) {
            Ok(true) => {}
            Ok(false) => emit(Event::Output(format!(
                "Session {} ({}) lost its process in the container reset and was not relaunched",
                session.panel_id, session.agent
            ))),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    if let Some(error) = failure {
        // Sessions reported before the failure stay reported when the reconnect retries.
        if marked {
            store.save(state)?;
        }
        return Err(error);
    }
    state.session_restart = None;
    store.save(state)
}

/// Whether the session's process runs again; `false` when the image cannot relaunch it
/// or the worker reports it lost.
fn relaunch(
    operation: OperationId,
    session: &Session,
    revision: &str,
    contract: &WorkerContract,
    run: &mut impl FnMut(&str) -> Result<String>,
) -> Result<bool> {
    if !contract.session_restart {
        return Ok(false);
    }
    match relaunch_status(&run(&relaunch_command(operation, session, revision)?)?) {
        Some(0 | 5) => Ok(true),
        Some(3 | 4) => Ok(false),
        _ => Err(Error::Invalid("A session could not be relaunched; reconnect to retry")),
    }
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
