//! One pull request as GitHub shows it to the person's token: what a native test of it may
//! rely on, read from GitHub and never from a worker.
use super::api::{SEARCH_LIMIT, valid_login, valid_repository};
use super::{Client, Error, Result, Secret};
use serde::Deserialize;

/// The fields of a pull request that decide whether Horizon may test its head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub open: bool,
    /// The head commit, 40 lowercase hexadecimal characters.
    pub head: String,
    /// `owner/name` of the head branch's repository, or `None` when that repository is gone.
    pub head_repository: Option<String>,
    /// The commit of the base branch that GitHub recorded at the last update of the pull
    /// request, which can be older than the branch now.
    pub base: String,
    /// The login of the author, such as `dependabot[bot]`.
    pub author: String,
}

impl Client {
    /// Pull request `number` of `repository` (`owner/name`).
    /// # Errors
    /// An invalid name or number, transport failures, a refused token, or a malformed answer.
    pub fn pull_request(&self, token: &Secret, repository: &str, number: u64) -> Result<PullRequest> {
        #[derive(Deserialize)]
        struct Fields {
            state: String,
            head: Branch,
            base: Branch,
            user: Author,
        }
        #[derive(Deserialize)]
        struct Branch {
            sha: String,
            repo: Option<Repository>,
        }
        #[derive(Deserialize)]
        struct Repository {
            full_name: String,
        }
        #[derive(Deserialize)]
        struct Author {
            login: String,
        }
        if !valid_repository(repository) || number == 0 {
            return Err(Error::InvalidResponse);
        }
        // A description can quote whole release notes, so the answer gets the search limit.
        let fields: Fields = self.request_limited(
            Some(token),
            &format!("/repos/{repository}/pulls/{number}"),
            SEARCH_LIMIT,
        )?;
        let head_repository = fields.head.repo.map(|repo| repo.full_name);
        if !commit(&fields.head.sha)
            || !commit(&fields.base.sha)
            || !bot_or_login(&fields.user.login)
            || head_repository.as_deref().is_some_and(|name| !valid_repository(name))
        {
            return Err(Error::InvalidResponse);
        }
        Ok(PullRequest {
            open: fields.state == "open",
            head: fields.head.sha,
            head_repository,
            base: fields.base.sha,
            author: fields.user.login,
        })
    }
}

fn commit(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A user login, or an app's bot login such as `dependabot[bot]`.
fn bot_or_login(login: &str) -> bool {
    valid_login(login.strip_suffix("[bot]").unwrap_or(login))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bot_login_and_a_full_commit_are_accepted_and_nothing_looser() {
        assert!(bot_or_login("dependabot[bot]"));
        assert!(bot_or_login("octocat"));
        assert!(!bot_or_login("[bot]"));
        assert!(!bot_or_login("evil name"));
        assert!(commit(&"a".repeat(40)));
        assert!(!commit(&"A".repeat(40)));
        assert!(!commit(&"a".repeat(39)));
    }
}
