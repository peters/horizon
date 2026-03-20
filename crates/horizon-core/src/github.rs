use std::path::{Path, PathBuf};
use std::process::Command;

use git2::Repository;
use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::task::TaskPrState;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct GitHubRepoRef {
    pub owner: String,
    pub name: String,
}

impl GitHubRepoRef {
    #[must_use]
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubWorkItemKind {
    Issue,
    PullRequest,
    ReviewComment,
}

impl GitHubWorkItemKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Issue => "Issue",
            Self::PullRequest => "PR",
            Self::ReviewComment => "Review",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct GitHubWorkItemRef {
    pub repo: GitHubRepoRef,
    pub kind: GitHubWorkItemKind,
    pub number: u64,
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_comment_id: Option<u64>,
}

impl GitHubWorkItemRef {
    #[must_use]
    pub fn label(&self) -> String {
        match self.kind {
            GitHubWorkItemKind::Issue => format!("Issue #{}", self.number),
            GitHubWorkItemKind::PullRequest => format!("PR #{}", self.number),
            GitHubWorkItemKind::ReviewComment => {
                let comment_id = self.review_comment_id.unwrap_or_default();
                format!("Review {} on PR #{}", comment_id, self.number)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRepoContext {
    pub repo_root: PathBuf,
    pub repo: GitHubRepoRef,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedGitHubWorkItem {
    pub work_item: GitHubWorkItemRef,
    pub workspace_name: String,
    pub repo_root: PathBuf,
    pub branch: Option<String>,
    pub pr_state: TaskPrState,
}

/// Discover the active GitHub repository context for a local path.
///
/// # Errors
///
/// Returns an error when the path is not inside a git repository, the
/// repository does not have an `origin` remote, or the remote is not a GitHub
/// remote that Horizon can resolve.
pub fn discover_repo_context(path: &Path) -> Result<GitHubRepoContext> {
    let repo = Repository::discover(path).map_err(|error| Error::Git(error.message().to_string()))?;
    let repo_root = repo.workdir().unwrap_or_else(|| repo.path()).to_path_buf();
    let remote = repo
        .find_remote("origin")
        .ok()
        .and_then(|remote| remote.url().map(str::to_string))
        .ok_or_else(|| Error::Git("missing GitHub origin remote".to_string()))?;
    let repo_ref = parse_remote_repo(&remote).ok_or_else(|| Error::Git("origin is not a GitHub remote".to_string()))?;
    Ok(GitHubRepoContext {
        repo_root,
        repo: repo_ref,
    })
}

/// Resolve user-provided GitHub work-item input into a concrete work item.
///
/// # Errors
///
/// Returns an error when the input is empty, does not match the requested
/// work-item kind, or `gh` cannot resolve the referenced GitHub item for the
/// active repository context.
pub fn resolve_work_item_input(
    kind: GitHubWorkItemKind,
    input: &str,
    repo_context: &GitHubRepoContext,
) -> Result<ResolvedGitHubWorkItem> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::GitHub(
            "enter a GitHub issue, PR, or review comment reference".to_string(),
        ));
    }

    match kind {
        GitHubWorkItemKind::Issue => resolve_issue(trimmed, repo_context),
        GitHubWorkItemKind::PullRequest => resolve_pull_request(trimmed, repo_context),
        GitHubWorkItemKind::ReviewComment => resolve_review_comment(trimmed, repo_context),
    }
}

/// Refresh the pull-request state associated with a task work item.
///
/// # Errors
///
/// Returns an error when `gh` cannot query the relevant pull-request metadata
/// for the work item or associated branch.
pub fn refresh_pull_request_status(work_item: &GitHubWorkItemRef, branch: Option<&str>) -> Result<TaskPrState> {
    match work_item.kind {
        GitHubWorkItemKind::PullRequest | GitHubWorkItemKind::ReviewComment => {
            let payload = gh_json(&[
                "pr",
                "view",
                &work_item.number.to_string(),
                "--repo",
                &work_item.repo.slug(),
                "--json",
                "number,state,isDraft",
            ])?;
            Ok(parse_pr_state(&payload))
        }
        GitHubWorkItemKind::Issue => {
            let Some(branch_name) = branch.filter(|value| !value.is_empty()) else {
                return Ok(TaskPrState::None);
            };
            let payload = gh_json(&[
                "pr",
                "list",
                "--repo",
                &work_item.repo.slug(),
                "--head",
                branch_name,
                "--limit",
                "1",
                "--json",
                "number,state,isDraft",
            ])?;
            let Some(first) = payload.as_array().and_then(|entries| entries.first()) else {
                return Ok(TaskPrState::None);
            };
            Ok(parse_pr_state(first))
        }
    }
}

fn resolve_issue(input: &str, repo_context: &GitHubRepoContext) -> Result<ResolvedGitHubWorkItem> {
    let (repo, number) = parse_numbered_ref(input, repo_context, GitHubWorkItemKind::Issue)?;
    let payload = gh_json(&[
        "issue",
        "view",
        &number.to_string(),
        "--repo",
        &repo.slug(),
        "--json",
        "number,title,url",
    ])?;
    let work_item = GitHubWorkItemRef {
        repo: repo.clone(),
        kind: GitHubWorkItemKind::Issue,
        number: payload_u64(&payload, "number")?,
        title: payload_string(&payload, "title")?,
        url: payload_string(&payload, "url")?,
        review_comment_id: None,
    };
    Ok(ResolvedGitHubWorkItem {
        workspace_name: format!("#{} {}", work_item.number, work_item.title),
        repo_root: repo_context.repo_root.clone(),
        branch: current_branch(&repo_context.repo_root),
        pr_state: TaskPrState::None,
        work_item,
    })
}

fn resolve_pull_request(input: &str, repo_context: &GitHubRepoContext) -> Result<ResolvedGitHubWorkItem> {
    let (repo, number) = parse_numbered_ref(input, repo_context, GitHubWorkItemKind::PullRequest)?;
    let payload = gh_json(&[
        "pr",
        "view",
        &number.to_string(),
        "--repo",
        &repo.slug(),
        "--json",
        "number,title,url,state,isDraft,headRefName",
    ])?;
    let work_item = GitHubWorkItemRef {
        repo: repo.clone(),
        kind: GitHubWorkItemKind::PullRequest,
        number: payload_u64(&payload, "number")?,
        title: payload_string(&payload, "title")?,
        url: payload_string(&payload, "url")?,
        review_comment_id: None,
    };
    Ok(ResolvedGitHubWorkItem {
        workspace_name: format!("PR #{} {}", work_item.number, work_item.title),
        repo_root: repo_context.repo_root.clone(),
        branch: payload_optional_string(&payload, "headRefName"),
        pr_state: parse_pr_state(&payload),
        work_item,
    })
}

fn resolve_review_comment(input: &str, repo_context: &GitHubRepoContext) -> Result<ResolvedGitHubWorkItem> {
    let (repo, comment_id) = parse_review_comment_ref(input, repo_context)?;
    let payload = gh_api_json(&format!("repos/{}/pulls/comments/{comment_id}", repo.slug()))?;
    let pr_number = parse_pr_number_from_url(&payload_string(&payload, "pull_request_url")?)?;

    let pr_payload = gh_json(&[
        "pr",
        "view",
        &pr_number.to_string(),
        "--repo",
        &repo.slug(),
        "--json",
        "number,title,url,state,isDraft,headRefName",
    ])?;
    let work_item = GitHubWorkItemRef {
        repo: repo.clone(),
        kind: GitHubWorkItemKind::ReviewComment,
        number: pr_number,
        title: payload_string(&pr_payload, "title")?,
        url: payload_string(&payload, "html_url")?,
        review_comment_id: Some(comment_id),
    };
    Ok(ResolvedGitHubWorkItem {
        workspace_name: format!("Review #{} {}", work_item.number, work_item.title),
        repo_root: repo_context.repo_root.clone(),
        branch: payload_optional_string(&pr_payload, "headRefName"),
        pr_state: parse_pr_state(&pr_payload),
        work_item,
    })
}

fn parse_pr_state(payload: &Value) -> TaskPrState {
    let number = payload.get("number").and_then(Value::as_u64).unwrap_or_default();
    let state = payload.get("state").and_then(Value::as_str).unwrap_or_default();
    let is_draft = payload.get("isDraft").and_then(Value::as_bool).unwrap_or(false);
    match state {
        "MERGED" => TaskPrState::Merged { number },
        "CLOSED" => TaskPrState::Closed { number },
        _ if is_draft => TaskPrState::Draft { number },
        _ if number > 0 => TaskPrState::Open { number },
        _ => TaskPrState::None,
    }
}

fn parse_numbered_ref(
    input: &str,
    repo_context: &GitHubRepoContext,
    kind: GitHubWorkItemKind,
) -> Result<(GitHubRepoRef, u64)> {
    if let Some((repo, number)) = parse_github_url(input, kind) {
        return Ok((repo, number));
    }

    if let Some((repo, number)) = parse_repo_slug_hash(input) {
        return Ok((repo, number));
    }

    let trimmed = input.trim_start_matches('#');
    trimmed
        .parse::<u64>()
        .map(|number| (repo_context.repo.clone(), number))
        .map_err(|_| Error::GitHub(format!("invalid {} reference: {input}", kind.label())))
}

fn parse_review_comment_ref(input: &str, repo_context: &GitHubRepoContext) -> Result<(GitHubRepoRef, u64)> {
    if let Some((repo, comment_id)) = parse_review_comment_url(input) {
        return Ok((repo, comment_id));
    }

    let trimmed = input.trim_start_matches('#').trim_start_matches("discussion_r");
    trimmed
        .parse::<u64>()
        .map(|comment_id| (repo_context.repo.clone(), comment_id))
        .map_err(|_| Error::GitHub(format!("invalid review comment reference: {input}")))
}

fn parse_github_url(input: &str, kind: GitHubWorkItemKind) -> Option<(GitHubRepoRef, u64)> {
    let pattern = match kind {
        GitHubWorkItemKind::Issue => r"^https://github\.com/([^/]+)/([^/]+)/issues/(\d+)",
        GitHubWorkItemKind::PullRequest => r"^https://github\.com/([^/]+)/([^/]+)/pull/(\d+)",
        GitHubWorkItemKind::ReviewComment => return None,
    };
    let regex = Regex::new(pattern).ok()?;
    let captures = regex.captures(input)?;
    let owner = captures.get(1)?.as_str().to_string();
    let name = captures.get(2)?.as_str().to_string();
    let number = captures.get(3)?.as_str().parse().ok()?;
    Some((GitHubRepoRef { owner, name }, number))
}

fn parse_review_comment_url(input: &str) -> Option<(GitHubRepoRef, u64)> {
    let regex = Regex::new(r"^https://github\.com/([^/]+)/([^/]+)/pull/\d+#discussion_r(\d+)").ok()?;
    let captures = regex.captures(input)?;
    let owner = captures.get(1)?.as_str().to_string();
    let name = captures.get(2)?.as_str().to_string();
    let comment_id = captures.get(3)?.as_str().parse().ok()?;
    Some((GitHubRepoRef { owner, name }, comment_id))
}

fn parse_repo_slug_hash(input: &str) -> Option<(GitHubRepoRef, u64)> {
    let regex = Regex::new(r"^([^/]+)/([^/#]+)#(\d+)$").ok()?;
    let captures = regex.captures(input)?;
    let owner = captures.get(1)?.as_str().to_string();
    let name = captures.get(2)?.as_str().to_string();
    let number = captures.get(3)?.as_str().parse().ok()?;
    Some((GitHubRepoRef { owner, name }, number))
}

fn parse_remote_repo(remote: &str) -> Option<GitHubRepoRef> {
    let trimmed = remote.trim_end_matches(".git");
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        let mut parts = rest.split('/');
        return Some(GitHubRepoRef {
            owner: parts.next()?.to_string(),
            name: parts.next()?.to_string(),
        });
    }
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        let mut parts = rest.split('/');
        return Some(GitHubRepoRef {
            owner: parts.next()?.to_string(),
            name: parts.next()?.to_string(),
        });
    }
    None
}

fn parse_pr_number_from_url(url: &str) -> Result<u64> {
    let Some(number) = url.rsplit('/').next() else {
        return Err(Error::GitHub(format!("invalid pull request URL: {url}")));
    };
    number
        .parse()
        .map_err(|_| Error::GitHub(format!("invalid pull request URL: {url}")))
}

fn current_branch(repo_root: &Path) -> Option<String> {
    let repo = Repository::discover(repo_root).ok()?;
    let head = repo.head().ok()?;
    head.shorthand().map(str::to_string)
}

fn gh_json(args: &[&str]) -> Result<Value> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .map_err(|error| Error::GitHub(format!("failed to run gh: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::GitHub(stderr.trim().to_string()));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| Error::GitHub(error.to_string()))
}

fn gh_api_json(path: &str) -> Result<Value> {
    gh_json(&["api", path])
}

fn payload_string(payload: &Value, key: &str) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::GitHub(format!("missing `{key}` in GitHub response")))
}

fn payload_optional_string(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(Value::as_str).map(str::to_string)
}

fn payload_u64(payload: &Value, key: &str) -> Result<u64> {
    payload
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::GitHub(format!("missing `{key}` in GitHub response")))
}

#[cfg(test)]
mod tests {
    use super::{GitHubRepoRef, GitHubWorkItemKind, parse_github_url, parse_remote_repo, parse_repo_slug_hash};

    #[test]
    fn parses_https_remote_repo() {
        assert_eq!(
            parse_remote_repo("https://github.com/peters/horizon.git"),
            Some(GitHubRepoRef {
                owner: "peters".to_string(),
                name: "horizon".to_string(),
            })
        );
    }

    #[test]
    fn parses_ssh_remote_repo() {
        assert_eq!(
            parse_remote_repo("git@github.com:peters/horizon.git"),
            Some(GitHubRepoRef {
                owner: "peters".to_string(),
                name: "horizon".to_string(),
            })
        );
    }

    #[test]
    fn parses_repo_hash_reference() {
        assert_eq!(
            parse_repo_slug_hash("peters/horizon#123"),
            Some((
                GitHubRepoRef {
                    owner: "peters".to_string(),
                    name: "horizon".to_string(),
                },
                123,
            ))
        );
    }

    #[test]
    fn parses_pull_request_url() {
        assert_eq!(
            parse_github_url(
                "https://github.com/peters/horizon/pull/77",
                GitHubWorkItemKind::PullRequest,
            ),
            Some((
                GitHubRepoRef {
                    owner: "peters".to_string(),
                    name: "horizon".to_string(),
                },
                77,
            ))
        );
    }
}
