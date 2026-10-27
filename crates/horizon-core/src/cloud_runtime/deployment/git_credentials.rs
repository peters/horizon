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
    // The create of an agent's cloud reads the GitHub access it gives its worker here, on
    // this deployment thread and never on the frame thread: the access reads this computer's
    // stored sign-in and calls GitHub. It is read now, and not when the create was answered,
    // because an image build can take minutes and the chain is installed with the worker that
    // the build produces. A person who asks the card to connect GitHub again signs in as
    // before.
    let mint = || -> Result<crate::cloud_runtime::github::Given> {
        use crate::cloud_runtime::github::{self, Given};
        if request.created_by.is_none() || request.renew_github {
            return Ok(Given::Person);
        }
        // Past the rent, a refusal must not fail the cloud: it continues without GitHub
        // access, and the card can connect it again.
        Ok(match github::for_agent_cloud(&request.settings, runner) {
            Ok(Some(access)) => Given::Stored(Box::new(access)),
            Ok(None) => Given::Unavailable(
                "This computer's GitHub policy allows a cloud without GitHub access, so this \
                 cloud has none. Connect GitHub from the cloud's card to give it access."
                    .into(),
            ),
            Err(refusal) => Given::Unavailable(refusal.to_string()),
        })
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
