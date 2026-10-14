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

/// Whether `a` and `b` name one repository on one server: the same host, the same path
/// whatever its case, and over one transport the same port. A transport's usual port counts
/// as none, so `git@github.com:acme/web` and `https://github.com/acme/web` are one
/// repository, while `https://git.example.com:8443/team/tool` is another server's than
/// `https://git.example.com/team/tool`. Ports of two transports are two services of one
/// server, such as SSH on 2222 beside HTTPS, so they are not compared.
#[must_use]
pub fn same_repository(a: &Remote, b: &Remote) -> bool {
    let (a_transport, a_port) = transport_and_port(&a.url);
    let (b_transport, b_port) = transport_and_port(&b.url);
    a.host == b.host
        && (a_transport != b_transport || a_port == b_port)
        && a.owner.eq_ignore_ascii_case(&b.owner)
        && a.name.eq_ignore_ascii_case(&b.name)
}

/// The transport of an address (`http` for both `http` and `https`, `ssh` for
/// `user@host:path` too) and the port it names, unless that is the scheme's usual one.
fn transport_and_port(url: &str) -> (&str, Option<&str>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return ("ssh", None);
    };
    let (transport, usual) = match scheme {
        "https" => ("http", "443"),
        "http" => ("http", "80"),
        "ssh" => ("ssh", "22"),
        "git" => ("git", "9418"),
        _ => (scheme, ""),
    };
    let port = rest
        .split('/')
        .next()
        .and_then(|authority| authority.rsplit('@').next())
        .and_then(|host| host.rsplit_once(':'))
        .map(|(_, port)| port)
        .filter(|port| *port != usual);
    (transport, port)
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
    named(clone_folder(parent, remote), portable(&remote.name))
}

/// The folder a clone of `remote` goes in: its owner's, unless a folder on the way there is
/// a checkout, as when GitHub's `acme/tools` is cloned and GitLab's `acme/tools/widget`
/// comes next, or is anything but a plain folder: a link, which would take the clone out of
/// `parent`, a file, or something that cannot be looked at. The clone then goes straight
/// under `parent`, never inside another repository or elsewhere.
fn clone_folder(parent: &Path, remote: &Remote) -> PathBuf {
    let mut folder = parent.to_owned();
    for segment in remote.owner.split('/') {
        folder.push(portable(segment));
        let usable = match std::fs::symlink_metadata(&folder) {
            // A `.git` there, or one that cannot be looked at, is a checkout to stay out of.
            Ok(meta) => {
                meta.is_dir()
                    && std::fs::symlink_metadata(folder.join(".git"))
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            }
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        };
        if !usable {
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
    // By the name as it was, which an earlier Horizon used as the folder's name.
    named(parent.to_owned(), remote.name.clone())
}

fn named(folder: PathBuf, name: String) -> impl Iterator<Item = PathBuf> {
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
    existing_checkout(parent, remote).map(|(path, _)| path)
}

/// As [`existing`], with whether the checkout is a linked worktree, told by the same reads
/// that find its origin.
#[must_use]
pub fn existing_checkout(parent: &Path, remote: &Remote) -> Option<(PathBuf, bool)> {
    candidates(parent, remote)
        .chain(earlier(parent, remote))
        .filter(|path| path.is_dir())
        .find_map(|path| {
            let (git, linked) = git_dirs(&path)?;
            let same = origin_in(&git)
                .and_then(|origin| parse(&origin))
                .is_some_and(|origin| origin.url == remote.url);
            (same && is_checkout(&path) && !transport::maybe_unfinished(&path)).then_some((path, linked))
        })
}

/// The name of the marker a clone keeps in a checkout's `.git` folder while it runs.
pub const CLONE_MARKER: &str = transport::MARKER_NAME;

/// Whether `marker`, the text of a [`CLONE_MARKER`] file, says the clone into its folder
/// was started and not finished. The caller reads the file however it must.
#[must_use]
pub fn clone_stopped(marker: &str) -> bool {
    transport::stopped(marker)
}

/// Whether a clone into `folder` was started and not finished, from at most a few kilobytes
/// of its marker file and without starting a process. Only a regular file is opened: a pipe
/// in its place would wait for a writer. A marker that is there but cannot be read, or is
/// too long, counts as a stopped clone, so a partial checkout is never taken for a finished one.
#[must_use]
pub fn clone_stopped_in(folder: &Path) -> bool {
    let marker = folder.join(".git").join(CLONE_MARKER);
    std::fs::symlink_metadata(&marker).is_ok() && clone_stopped(&small_text(&marker, 4096).unwrap_or_default())
}

/// The text of the regular file at `path`, links followed, read only when it is at most
/// `limit` bytes. A pipe or device in a checkout's place is never opened, since opening it
/// could wait for ever, and a large file is not read into memory.
fn small_text(path: &Path, limit: u64) -> Option<String> {
    use std::io::Read as _;
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    // A file that grew since it was measured is still read no further than the limit.
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit.saturating_add(1))
        .read_to_string(&mut text)
        .ok()?;
    (u64::try_from(text.len()).ok()? <= limit).then_some(text)
}

/// The most bytes of a checkout's config read to find its origin.
const CONFIG_LIMIT: u64 = 1024 * 1024;

/// The address a checkout's `origin` was cloned from, read from the config file of its
/// common Git directory `git`: a folder is looked at without starting a process, so a
/// search can run while a dialog is drawn.
fn origin_in(git: &Path) -> Option<String> {
    let config = small_text(&git.join("config"), CONFIG_LIMIT)?;
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

/// The Git directory that holds a checkout's configuration, and whether the checkout is a
/// linked worktree: its `.git` file points at its own directory, which names the common one
/// in `commondir`. Read from files only.
fn git_dirs(checkout: &Path) -> Option<(PathBuf, bool)> {
    let dot_git = checkout.join(".git");
    if dot_git.is_dir() {
        return Some((dot_git, false));
    }
    let pointer = small_text(&dot_git, 4096)?;
    let own = checkout.join(pointer.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim());
    match small_text(&own.join("commondir"), 4096) {
        Some(common) => Some((own.join(common.trim()), true)),
        None => Some((own, false)),
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
