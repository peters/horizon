//! Git credential binding transferred to the worker for the deployed repository.
use super::{Connection, Result, Runner};
use std::time::Duration;

/// Installs the repository's Git binding, or removes any earlier one from the worker,
/// then gives the worker's GitHub service current access when GitHub is connected.
pub(super) fn configure_git_auth(
    git_auth: Option<crate::cloud_runtime::git_auth::Prepared>,
    github: Option<&crate::cloud_runtime::github::Settings>,
    state: &crate::cloud_runtime::state::Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    let legacy = git_auth.is_some();
    if let Some(git_auth) = git_auth {
        git_auth.install(connection, runner)?;
    } else {
        runner.run(
            "Git credential removal",
            &mut connection.command(
                "if command -v horizon-worker-git-auth >/dev/null 2>&1; then horizon-worker-git-auth clear; fi",
            ),
            Duration::from_secs(20),
        )?;
    }
    crate::cloud_runtime::github::configure(github, state, connection, runner, legacy)
}
