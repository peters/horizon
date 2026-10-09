//! Git credential binding transferred to the worker for the deployed repository.
use super::{Connection, Result, Runner};
use std::time::Duration;

/// Gives the worker's GitHub service current access when GitHub is connected. Without
/// that access the worker gets the repository's Git binding from cloud settings; with
/// it, or without a binding, any earlier binding is removed from the worker.
pub(super) fn configure_git_auth(
    git_auth: Option<crate::cloud_runtime::git_auth::Prepared>,
    github: Option<&crate::cloud_runtime::github::Settings>,
    state: &crate::cloud_runtime::state::Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    let legacy = git_auth.is_some();
    let served = crate::cloud_runtime::github::configure(github, state, connection, runner, legacy)?;
    match git_auth {
        Some(git_auth) if !served => git_auth.install(connection, runner),
        _ => runner
            .run(
                "Git credential removal",
                &mut connection.command(
                    "if command -v horizon-worker-git-auth >/dev/null 2>&1; then horizon-worker-git-auth clear; fi",
                ),
                Duration::from_secs(20),
            )
            .map(drop),
    }
}
