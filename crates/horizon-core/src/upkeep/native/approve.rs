//! Whether Horizon may start a test cloud for a request, decided from GitHub and the
//! repository, never from what the worker wrote beyond the request's three fields.
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::Request;
use crate::cloud_runtime::{Cancellation, command::Runner, github::host, repository, settings::Settings};
use crate::upkeep::worker::Config;

const GIT_LIMIT: Duration = Duration::from_secs(60);
/// The folder whose files make the test cloud's image.
const RECIPE: &str = ".horizon";

/// What a test cloud starts from.
#[derive(Clone, Debug)]
pub struct Approved {
    /// The checkout of the repository on this computer, which holds the head commit now.
    pub checkout: PathBuf,
    pub revision: String,
    pub profile_name: String,
    pub profile: horizon_cloud::Profile,
}

/// Checks `request` against GitHub, read with the GitHub sign-in of Cloud settings in
/// `root`, and against the repository, and gets its head commit into the checkout on this
/// computer. Blocks on the network, Git and the disk: call it off the UI thread.
/// # Errors
/// Why Horizon starts no test cloud, in a sentence for the person and the agent.
pub fn approve(root: &Path, request: &Request, config: &Config, cancel: &Cancellation) -> Result<Approved, String> {
    // The request waited: the person may have removed the repository from the Upkeep since.
    if !config
        .repositories
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&request.repository))
    {
        return Err(format!(
            "{} is no longer in the list of this Upkeep.",
            request.repository
        ));
    }
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    let github = settings
        .github
        .as_ref()
        .ok_or("Connect GitHub in Cloud settings: Horizon reads the pull request with it.")?;
    let (_, token) = host::current(root, github)
        .map_err(|error| format!("{error}."))?
        .ok_or("Connect GitHub in Cloud settings: Horizon reads the pull request with it.")?;
    let pull = horizon_cloud::github::Client::new()
        .pull_request(&token, &request.repository, request.number)
        .map_err(|error| format!("GitHub did not show {}: {error}", request.label()))?;
    eligible(&pull, request, config)?;
    let checkout = crate::upkeep::checkout(root, &settings, &request.repository)?;
    let remote = repository::source::parse(&request.repository).ok_or("The repository is not on GitHub.")?;
    let git_token = repository::source::Token::new(&remote, token.expose());
    repository::source::fetch_commits(
        &remote,
        &checkout,
        &[&pull.head, &pull.base],
        git_token.as_ref(),
        cancel,
    )
    .map_err(|failure| format!("Fetching the head of {} failed: {failure}", request.label()))?;
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    recipe_unchanged(&checkout, &pull.base, &pull.head, &runner)?;
    let config = repository::launch::committed_config(&checkout, &pull.head, &runner)
        .map_err(|error| format!("{error}."))?
        .ok_or("The repository has no .horizon/cloud.yml, so Horizon has no image with its toolchain.")?;
    let profile = config
        .profiles
        .get(&config.default)
        .ok_or("The default profile of .horizon/cloud.yml is missing.")?
        .clone();
    usable(&profile)?;
    Ok(Approved {
        checkout,
        revision: pull.head,
        profile_name: config.default,
        profile,
    })
}

/// The checks that GitHub's answer alone decides.
fn eligible(pull: &horizon_cloud::github::PullRequest, request: &Request, config: &Config) -> Result<(), String> {
    if !pull.open {
        return Err(format!("{} is not open.", request.label()));
    }
    if pull.head != request.head {
        return Err(format!(
            "The head of {} is {} now, not the requested commit.",
            request.label(),
            &pull.head[..12]
        ));
    }
    if !pull
        .head_repository
        .as_deref()
        .is_some_and(|name| name.eq_ignore_ascii_case(&request.repository))
    {
        return Err(format!(
            "The head branch of {} is in another repository; Horizon tests only branches of the repository itself.",
            request.label()
        ));
    }
    if !config
        .authors
        .iter()
        .any(|author| login_of(author).eq_ignore_ascii_case(&pull.author))
    {
        return Err(format!(
            "{} was opened by {}, not by a bot of this Upkeep.",
            request.label(),
            pull.author
        ));
    }
    Ok(())
}

/// The login GitHub shows for an Upkeep author: `app/name` is the bot `name[bot]`.
fn login_of(author: &str) -> String {
    author
        .strip_prefix("app/")
        .map_or_else(|| author.to_owned(), |app| format!("{app}[bot]"))
}

/// Refuses a pull request that changes a file in `.horizon` since it left the base branch:
/// Docker on this computer then builds only files that were reviewed on that branch.
fn recipe_unchanged(checkout: &Path, base: &str, head: &str, runner: &Runner<'_>) -> Result<(), String> {
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        command.arg("--no-replace-objects").arg("-C").arg(checkout).args(args);
        command
    };
    let fork = runner
        .run("Find the merge base", &mut git(&["merge-base", base, head]), GIT_LIMIT)
        .map_err(|error| format!("The head and base of the pull request share no history: {error}."))?;
    let changed = runner
        .run(
            "List changed recipe files",
            &mut git(&["diff", "--no-ext-diff", "--name-only", fork.trim(), head, "--", RECIPE]),
            GIT_LIMIT,
        )
        .map_err(|error| format!("Comparing .horizon failed: {error}."))?;
    if changed.trim().is_empty() {
        Ok(())
    } else {
        Err("The pull request changes files in .horizon. Horizon builds test images only from reviewed recipes; qualify it by hand.".into())
    }
}

/// Refuses a profile that a test cloud cannot run, or whose image would be built from files
/// outside `.horizon`, which the pull request may change.
fn usable(profile: &horizon_cloud::Profile) -> Result<(), String> {
    if !profile.capabilities.desktop {
        return Err("The default profile has no desktop, so nobody could watch the test.".into());
    }
    if agent(profile).is_none() || !profile.capabilities.agent_approvals.is_auto() {
        return Err("The default profile runs no Codex or Claude agent without approvals, so no agent could run the test alone.".into());
    }
    if profile.gpu {
        return Err("The default profile needs a GPU; Horizon starts test clouds on Hetzner, without one.".into());
    }
    if let Some(build) = &profile.build
        && !(inside_recipe(&build.context) && inside_recipe(&build.dockerfile))
    {
        return Err("The image of the default profile is built from files outside .horizon.".into());
    }
    Ok(())
}

/// The panel of the agent that runs a test on a cloud of `profile`: Codex when the profile has
/// it, else Claude.
#[must_use]
pub fn agent(profile: &horizon_cloud::Profile) -> Option<crate::PanelKind> {
    let has = |agent| profile.capabilities.agents.contains(&agent);
    if has(horizon_cloud::Agent::Codex) {
        Some(crate::PanelKind::Codex)
    } else if has(horizon_cloud::Agent::Claude) {
        Some(crate::PanelKind::Claude)
    } else {
        None
    }
}

/// Whether `path`, relative to the repository, names `.horizon` or a path below it.
fn inside_recipe(path: &str) -> bool {
    let mut components = Path::new(path)
        .components()
        .filter(|component| *component != Component::CurDir);
    components.next() == Some(Component::Normal(RECIPE.as_ref()))
        && components.all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(author: &str) -> horizon_cloud::github::PullRequest {
        horizon_cloud::github::PullRequest {
            open: true,
            head: "a".repeat(40),
            head_repository: Some("Acme/Web".into()),
            base: "b".repeat(40),
            author: author.into(),
        }
    }

    fn request() -> Request {
        Request {
            repository: "acme/web".into(),
            number: 7,
            head: "a".repeat(40),
            reason: String::new(),
        }
    }

    fn config() -> Config {
        Config {
            repositories: vec!["acme/web".into()],
            authors: vec!["app/dependabot".into()],
            ..Config::default()
        }
    }

    #[test]
    fn only_an_open_bot_pull_request_of_the_repository_at_the_asked_head_is_eligible() {
        assert_eq!(eligible(&pull("dependabot[bot]"), &request(), &config()), Ok(()));
        assert!(
            eligible(&pull("someone"), &request(), &config()).is_err(),
            "a person's pull request"
        );
        let mut moved = pull("dependabot[bot]");
        moved.head = "c".repeat(40);
        assert!(eligible(&moved, &request(), &config()).is_err(), "the head moved on");
        let mut fork = pull("dependabot[bot]");
        fork.head_repository = Some("evil/web".into());
        assert!(eligible(&fork, &request(), &config()).is_err(), "a branch of a fork");
        let mut gone = pull("dependabot[bot]");
        gone.head_repository = None;
        assert!(
            eligible(&gone, &request(), &config()).is_err(),
            "a deleted head repository"
        );
        let mut closed = pull("dependabot[bot]");
        closed.open = false;
        assert!(eligible(&closed, &request(), &config()).is_err());
    }

    #[test]
    fn only_a_change_of_horizon_on_the_pull_request_branch_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        let write = |path: &str, text: &str| {
            let path = repo.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        git(&["init", "-q", "-b", "main"]);
        write(".horizon/Dockerfile", "FROM base\n");
        write("src/app.txt", "1\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        git(&["checkout", "-qb", "bump"]);
        write("src/app.txt", "2\n");
        git(&["commit", "-qam", "bump"]);
        let bump = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-qb", "recipe"]);
        write(".horizon/Dockerfile", "FROM evil\n");
        git(&["commit", "-qam", "recipe"]);
        let recipe = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "main"]);
        write(".horizon/Dockerfile", "FROM newer\n");
        git(&["commit", "-qam", "main moved on"]);
        let main = git(&["rev-parse", "HEAD"]);
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        assert_eq!(
            recipe_unchanged(repo, &main, &bump, &runner),
            Ok(()),
            "a newer recipe on the base branch is not a change of the pull request"
        );
        assert!(recipe_unchanged(repo, &main, &recipe, &runner).is_err());
    }

    #[test]
    fn a_recipe_must_stay_inside_horizon() {
        assert!(inside_recipe(".horizon"));
        assert!(inside_recipe("./.horizon/Dockerfile"));
        assert!(!inside_recipe("."));
        assert!(!inside_recipe(".horizon/../Dockerfile"));
        assert!(!inside_recipe("/.horizon"));
        assert!(!inside_recipe("src/.horizon"));
    }
}
