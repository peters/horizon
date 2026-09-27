//! Git credential binding transferred to the worker for the deployed repository.
use super::{Connection, Result, Runner};
use std::time::Duration;

/// Installs the repository's Git binding, or removes any earlier one from the worker.
pub(super) fn configure_git_auth(
    git_auth: Option<crate::cloud_runtime::git_auth::Prepared>,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    if let Some(git_auth) = git_auth {
        return git_auth.install(connection, runner);
    }
    runner.run(
        "Git credential removal",
        &mut connection
            .command("if command -v horizon-worker-git-auth >/dev/null 2>&1; then horizon-worker-git-auth clear; fi"),
        Duration::from_secs(20),
    )?;
    Ok(())
}
