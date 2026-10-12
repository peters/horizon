//! The GitHub git sources that a commit's Cargo lock files pin, read through libgit2.
use super::super::super::pins::Commit;
use std::path::Path;

/// The largest lock file read.
const LOCK_LIMIT: u64 = 2 * 1024 * 1024;

/// How far the search for lock files goes: folders down, tree entries and lock files.
pub(super) const LOCK_WALK: (usize, usize, usize) = (8, 100_000, 64);

/// The most git sources read from the lock files.
pub(super) const SOURCE_LIMIT: usize = 64;

/// One git source of a lock file: its address as the lock writes it, the GitHub
/// repository and the locked commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Source {
    pub(super) address: String,
    pub(super) owner: String,
    pub(super) name: String,
    pub(super) commit: String,
}

impl Source {
    /// `owner/name` in lower case: GitHub names one repository in any case.
    pub(super) fn key(&self) -> String {
        format!("{}/{}", self.owner, self.name).to_ascii_lowercase()
    }
}

/// The GitHub repositories the Cargo lock files in `commit` pin, as `owner/name` in
/// lower case, sorted and once each: what the box names and the person approves.
pub(in crate::app::cloud_panel::production::repository_setup) fn locked_repositories(
    repository: &Path,
    commit: &str,
) -> Vec<String> {
    let mut keys: Vec<String> = locked_sources(repository, commit).0.iter().map(Source::key).collect();
    keys.sort();
    keys.dedup();
    keys
}

/// The git sources of the Cargo lock files in `commit`, anywhere up to a bounded depth,
/// without duplicates, read through libgit2 (see [`Commit`]), and whether a limit left
/// lock files unread.
pub(super) fn locked_sources(repository: &Path, commit: &str) -> (Vec<Source>, bool) {
    let Some(commit) = Commit::open(repository, commit) else {
        return (Vec::new(), false);
    };
    let (locks, mut cut) = commit.files_named("Cargo.lock", LOCK_WALK);
    let mut sources = Vec::new();
    for lock in locks {
        // A lock file too large to read is one the search did not read: say so.
        let Some(text) = commit.blob(&lock, LOCK_LIMIT) else {
            cut = true;
            continue;
        };
        for source in String::from_utf8_lossy(&text).lines().filter_map(git_source) {
            if sources.contains(&source) {
                continue;
            }
            if sources.len() == SOURCE_LIMIT {
                cut = true;
                break;
            }
            sources.push(source);
        }
    }
    (sources, cut)
}

/// `source = "git+https://github.com/OWNER/NAME?rev=...#SHA"` locks commit `SHA` of
/// GitHub repository `OWNER/NAME`; an SSH address to github.com counts too, any other
/// host does not.
pub(super) fn git_source(line: &str) -> Option<Source> {
    let url = line.trim().strip_prefix("source = \"git+")?.strip_suffix('"')?;
    let (location, commit) = url.rsplit_once('#')?;
    let address = location.split('?').next()?;
    let path = github_path(address)?;
    let (owner, name) = path.trim_end_matches('/').split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    // An owner is letters, digits and dashes; a name may also hold `.` and `_` and start
    // with a dot, as `.github` does, but is never `.` or `..`.
    let allowed = |part: &str| {
        part.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    // GitHub's own limits: an owner of at most 39 characters, a name of at most 100.
    let owner_ok = (1..=39).contains(&owner.len()) && !owner.starts_with(['.', '-']) && allowed(owner);
    let name_ok = !matches!(name, "" | "." | "..") && name.len() <= 100 && !name.starts_with('-') && allowed(name);
    let commit_ok = commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit());
    (owner_ok && name_ok && commit_ok).then(|| Source {
        address: address.to_owned(),
        owner: owner.to_owned(),
        name: name.to_owned(),
        commit: commit.to_ascii_lowercase(),
    })
}

/// The `OWNER/NAME...` path of an `https://github.com/` or `ssh://git@github.com/`
/// address. The scheme and the host match in any case, as URLs and DNS names do; the
/// address itself stays as written, since Cargo fetches that exact text.
fn github_path(address: &str) -> Option<&str> {
    let (scheme, rest) = address.split_once("://")?;
    let rest = if scheme.eq_ignore_ascii_case("https") {
        rest
    } else if scheme.eq_ignore_ascii_case("ssh") {
        rest.strip_prefix("git@")?
    } else {
        return None;
    };
    let (host, path) = rest.split_once('/')?;
    host.eq_ignore_ascii_case("github.com").then_some(path)
}
