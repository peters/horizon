//! Git credential binding transferred to the worker for the deployed repository.
use super::{Connection, Request, Result, Runner};
use std::time::Duration;

/// Gives the worker's GitHub service current access when GitHub is connected. Without
/// that access the worker gets the repository's Git binding from cloud settings; with
/// it, or without a binding, any earlier binding is removed from the worker.
pub(super) fn configure_git_auth(
    git_auth: Option<crate::cloud_runtime::git_auth::Prepared>,
    request: &Request,
    state: &crate::cloud_runtime::state::Deployment,
    connection: &Connection,
    runner: &Runner<'_>,
) -> Result<()> {
    let legacy = git_auth.is_some();
    let github = request.settings.github.as_ref();
    // The create of an agent's cloud mints the app's installation token here, on this
    // deployment thread and never on the frame thread: minting reads the app's private key
    // file and calls GitHub. It is minted now, and not when the create was answered, because
    // the worker refuses a token that ended and an image build may outlast the hour a token
    // lasts. A person who asks the card to connect GitHub again signs in as before.
    let mint = || -> Result<Option<crate::cloud_runtime::github::Installation>> {
        if request.created_by.is_some() && !request.renew_github {
            return Ok(crate::cloud_runtime::github::for_agent_cloud(
                &request.settings,
                runner,
            )?);
        }
        Ok(None)
    };
    let served = crate::cloud_runtime::github::configure(
        github,
        &mint,
        state,
        connection,
        runner,
        legacy,
        request.renew_github,
    )?;
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
