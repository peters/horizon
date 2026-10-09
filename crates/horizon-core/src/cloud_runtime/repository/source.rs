//! Turn a pasted Git URL into a local checkout, using the credentials Git already has.
//!
//! Nothing here stores a secret. The clone runs the system `git` with prompts disabled, so a
//! configured credential helper, `gh` login, SSH agent or key works unchanged and everything
//! else fails fast as [`Failure::SignIn`]. The caller then asks for a personal access token,
//! which serves that one clone and may be handed to Git's own credential helper to keep.
use base64::Engine;
use horizon_cloud::Cancellation;
mod transport;

pub use transport::{Progress, Snapshot, clone, discard, probe, resumable};

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// A repository named by a URL, shorthand or browser address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Remote {
    pub url: String,
    pub host: String,
    /// The path before the name: the owner on GitHub, a group and its subgroups on GitLab.
    /// A clone goes in a folder of that path, so two owners' repositories of one name never
    /// meet.
    pub owner: String,
    pub name: String,
}

/// Local folders that usually hold a person's checkouts, in order of preference.
const CHECKOUT_FOLDERS: [&str; 5] = ["github", "code", "src", "projects", "dev"];

/// Reads `https://host/group/repo(.git)`, `git@host:group/repo`, `host/group/repo` and
/// `owner/repo` (GitHub). Returns `None` for anything else, including local paths.
#[must_use]
pub fn parse(input: &str) -> Option<Remote> {
    let input = input.trim();
    if input.is_empty() || input.starts_with(['-', '/', '~', '.']) || input.contains(char::is_whitespace) {
        return None;
    }
    let (host, path, prefix) = if let Some((scheme, rest)) = input.split_once("://") {
        if !["https", "http", "ssh", "git"].contains(&scheme) {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        // A password or token in the address would reach argv and the checkout's config.
        let (userinfo, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
        let user = if scheme == "ssh" {
            userinfo.split(':').next().unwrap_or_default()
        } else {
            ""
        };
        if !user.is_empty() && !valid_name(user) {
            return None;
        }
        let at = if user.is_empty() {
            String::new()
        } else {
            format!("{user}@")
        };
        // Host names are not case sensitive; one spelling keeps a link, its token and its checkout together.
        let hostport = hostport.to_ascii_lowercase();
        (host_of(&hostport), path, format!("{scheme}://{at}{hostport}/"))
    } else if let Some((account, path)) = input.split_once(':')
        && let Some((user, host)) = account.split_once('@')
        && valid_name(user)
    {
        let host = host.to_ascii_lowercase();
        (host.clone(), path, format!("{user}@{host}:"))
    } else {
        let (first, rest) = input.split_once('/')?;
        if first.contains('.') && rest.contains('/') {
            let host = first.to_ascii_lowercase();
            (host.clone(), rest, format!("https://{host}/"))
        } else if !first.contains('.') && !rest.contains('/') {
            ("github.com".to_owned(), input, "https://github.com/".to_owned())
        } else {
            return None;
        }
    };
    let path = repository_path(&host, path);
    let (owner, name) = path.rsplit_once('/')?;
    if !valid_host(&host) || !path.split('/').all(valid_segment) {
        return None;
    }
    Some(Remote {
        url: format!("{prefix}{path}.git"),
        host,
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

/// A host name Git can be handed safely: dotted, and never something it would read as an option.
fn valid_host(host: &str) -> bool {
    host.contains('.') && valid_name(host)
}

/// One folder of the repository's path, which also names the clone's folder: no separators,
/// drive letters or dot components, whatever the platform.
fn valid_segment(segment: &str) -> bool {
    !matches!(segment, "" | "." | "..")
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_name(name: &str) -> bool {
    !name.starts_with(['-', '.'])
        && !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn host_of(authority: &str) -> String {
    let host = authority.rsplit('@').next().unwrap_or(authority);
    host.split(':').next().unwrap_or(host).to_owned()
}

/// The repository's own path: browser suffixes such as `/tree/main` or GitLab's `/-/…` are dropped.
fn repository_path(host: &str, path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or(path).trim_matches('/');
    let path = path.split("/-/").next().unwrap_or(path);
    let path = if host == "github.com" {
        path.splitn(3, '/').take(2).collect::<Vec<_>>().join("/")
    } else {
        path.to_owned()
    };
    path.trim_end_matches(".git").to_owned()
}

/// Where clones go unless the person chooses: the folder they already keep code in.
#[must_use]
pub fn default_parent(home: &Path) -> PathBuf {
    CHECKOUT_FOLDERS
        .iter()
        .map(|name| home.join(name))
        .find(|path| path.is_dir())
        .unwrap_or_else(|| home.join("Horizon"))
}

/// How many folders a clone of one repository can land in under its owner's folder: the
/// repository's name, then that name with `-2`, `-3` and so on. A search for an earlier
/// clone looks in every one of them, and a new clone never goes beyond them, so the two
/// always agree.
const CANDIDATES: u32 = 25;

/// The folders a clone of `remote` can land in, `parent/<owner>/<name>` first, in the
/// order they are tried.
fn candidates<'a>(parent: &'a Path, remote: &'a Remote) -> impl Iterator<Item = PathBuf> + 'a {
    named(clone_folder(parent, remote), &remote.name)
}

/// The folder a clone of `remote` goes in: its owner's, unless a folder on the way there is
/// a checkout, as when GitHub's `acme/tools` is cloned and GitLab's `acme/tools/widget`
/// comes next. A clone never lands inside another repository, so it then goes straight
/// under `parent`.
fn clone_folder(parent: &Path, remote: &Remote) -> PathBuf {
    let mut folder = parent.to_owned();
    for segment in remote.owner.split('/') {
        folder.push(portable(segment));
        if folder.join(".git").exists() {
            return parent.to_owned();
        }
    }
    folder
}

/// A folder name for `segment` that every platform can make: Windows reserves device names
/// such as `CON` or `com1.txt` and drops a trailing dot, so those get a `_`.
fn portable(segment: &str) -> String {
    let stem = segment.split('.').next().unwrap_or(segment).to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.ends_with(|c: char| ('1'..='9').contains(&c)));
    if device || segment.ends_with('.') {
        format!("{segment}_")
    } else {
        segment.to_owned()
    }
}

/// Where clones went before they were kept by owner, `parent/<name>` first: only searched,
/// so an earlier clone is found again, and never cloned into.
fn earlier<'a>(parent: &'a Path, remote: &'a Remote) -> impl Iterator<Item = PathBuf> + 'a {
    named(parent.to_owned(), &remote.name)
}

fn named(folder: PathBuf, name: &str) -> impl Iterator<Item = PathBuf> {
    let name = portable(name);
    (1..=CANDIDATES).map(move |n| match n {
        1 => folder.join(&name),
        n => folder.join(format!("{name}-{n}")),
    })
}

/// A folder under `parent/<owner>` named for the repository that nothing occupies yet.
#[must_use]
pub fn destination(parent: &Path, remote: &Remote) -> PathBuf {
    candidates(parent, remote)
        .find(|path| !path.exists())
        .unwrap_or_else(|| clone_folder(parent, remote).join(portable(&remote.name)))
}

/// The checkout of `remote` already under `parent`, so a second request reuses it: any of the
/// folders [`destination`] can choose from, then those of the earlier layout without the
/// owner's folder, each asked about its origin only if it exists.
#[must_use]
pub fn existing(parent: &Path, remote: &Remote) -> Option<PathBuf> {
    candidates(parent, remote)
        .chain(earlier(parent, remote))
        .filter(|path| path.is_dir())
        .find(|path| {
            origin_url(path)
                .and_then(|origin| parse(&origin))
                .is_some_and(|origin| origin.url == remote.url)
                && is_checkout(path)
                && !transport::unfinished(path)
        })
}

/// The address a checkout's `origin` was cloned from, read from its config file: a folder is
/// looked at without starting a process, so a search can run while a dialog is drawn.
fn origin_url(checkout: &Path) -> Option<String> {
    let config = std::fs::read_to_string(common_git_dir(checkout)?.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_origin = line.starts_with("[remote \"origin\"]");
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}

/// The Git directory that holds a checkout's configuration. A linked worktree has a `.git` file
/// that points at its own directory, which in turn names the common one.
fn common_git_dir(checkout: &Path) -> Option<PathBuf> {
    let dot_git = checkout.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let pointer = std::fs::read_to_string(&dot_git).ok()?;
    let own = checkout.join(pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim());
    match std::fs::read_to_string(own.join("commondir")) {
        Ok(common) => Some(own.join(common.trim())),
        Err(_) => Some(own),
    }
}

fn git_output(path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A personal access token for one host. It reaches Git only through the environment of a
/// single clone, never argv or disk, unless the person asks Git's credential helper to keep it.
pub struct Token {
    username: &'static str,
    secret: zeroize::Zeroizing<String>,
    /// `scheme://host[:port]/` of the repository it was pasted for: Git sends the header there only.
    scope: String,
}

impl Token {
    #[must_use]
    pub fn new(remote: &Remote, secret: &str) -> Option<Self> {
        let secret = secret.trim();
        let origin: Vec<&str> = remote.url.splitn(4, '/').collect();
        let (true, [scheme, "", authority, _]) =
            (secret.bytes().all(|byte| byte.is_ascii_graphic()), origin.as_slice())
        else {
            return None;
        };
        (!secret.is_empty() && *scheme == "https:").then(|| Self {
            username: if remote.host == "github.com" {
                "x-access-token"
            } else {
                "oauth2"
            },
            secret: zeroize::Zeroizing::new(secret.to_owned()),
            scope: format!("https://{authority}/"),
        })
    }

    fn header(&self) -> String {
        let pair = zeroize::Zeroizing::new(format!("{}:{}", self.username, self.secret.as_str()));
        format!(
            "Authorization: Basic {}",
            base64::engine::general_purpose::STANDARD.encode(pair.as_bytes())
        )
    }
}

/// Whether the `git config --get-regexp` `listing` leaves a credential helper in force for the
/// origin `scope`: the last applicable setting decides, and an empty one is Git's way to switch
/// the helpers before it off.
fn helper_configured(listing: &str, scope: &str) -> bool {
    let mut configured = false;
    for line in listing.lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let applies = match key
            .strip_prefix("credential.")
            .and_then(|rest| rest.strip_suffix(".helper"))
        {
            Some(url) => scope.starts_with(url),
            None => key == "credential.helper",
        };
        if applies {
            configured = !value.trim().is_empty();
        }
    }
    configured
}

/// The longest Git's credential helper is given to keep a token.
const REMEMBER_LIMIT: Duration = Duration::from_secs(10);

/// Asks the person's configured Git credential helper to keep `token` for the host it was pasted for.
/// Returns whether a helper is configured to receive it.
#[must_use]
pub fn remember(token: &Token, cancel: &Cancellation) -> bool {
    let configured = git_output(
        Path::new("."),
        &["config", "--get-regexp", r"^credential\.(.*\.)?helper$"],
    )
    .is_some_and(|listing| helper_configured(&listing, &token.scope));
    if !configured {
        return false;
    }
    let Ok(mut child) = Command::new("git")
        .args(["credential", "approve"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = write!(
            stdin,
            "protocol=https\nhost={}\nusername={}\npassword={}\n\n",
            token.scope.trim_start_matches("https://").trim_end_matches('/'),
            token.username,
            token.secret.as_str()
        );
    }
    // A helper that waits for an unlock prompt is given a while, and never past a cancel.
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if !cancel.is_cancelled() && started.elapsed() < REMEMBER_LIMIT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Failure {
    /// Git needs a credential it does not have; an interactive `git clone` can ask for it.
    #[error("Sign in to {0} to read this repository.")]
    SignIn(String),
    #[error("Repository not found. Check the address, and that this account can read it.")]
    NotFound,
    #[error("Cannot reach the host. Check the connection and retry.")]
    Network,
    #[error("Clone cancelled.")]
    Cancelled,
    /// A clone that stopped after receiving part of the repository, which it kept.
    #[error("{0}")]
    Interrupted(String),
    #[error("{0}")]
    Other(String),
}

fn classify(remote: &Remote, stderr: &str) -> Failure {
    let text = stderr.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    let web = remote.url.starts_with("https://");
    let plain = remote.url.starts_with("http://");
    let refused = has(&[
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "requested url returned error: 401",
        "requested url returned error: 403",
    ]);
    if web && refused {
        Failure::SignIn(remote.host.clone())
    } else if plain && refused {
        // A token is never sent over plain http, so asking for one would lead nowhere.
        Failure::Other(format!(
            "{} asks for a sign-in but this link is plain http, so no token can be sent. Use its https link.",
            remote.host
        ))
    } else if !web && !plain && (refused || has(&["permission denied (", "host key verification failed"])) {
        // A token cannot help over SSH: the key, or trust in the host, is what is missing.
        Failure::Other(format!(
            "Git could not sign in over SSH. Give your SSH key access to this repository and trust {} once with `ssh`, or paste its https link.",
            remote.host
        ))
    } else if has(&["not found", "does not exist", "returned error: 404"]) {
        // Hosts answer a private repository, or one a stale credential cannot read, the same way
        // as a missing one; only for https can a token change that.
        if web {
            Failure::SignIn(remote.host.clone())
        } else {
            Failure::NotFound
        }
    } else if has(&[
        "could not resolve host",
        "timed out",
        "unreachable",
        "connection refused",
        "unable to access",
    ]) {
        Failure::Network
    } else {
        Failure::Other(
            stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("git clone failed")
                .trim()
                .to_owned(),
        )
    }
}

/// Whether `path` holds a finished checkout, as the interactive clone leaves one.
#[must_use]
pub fn is_checkout(path: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests;
