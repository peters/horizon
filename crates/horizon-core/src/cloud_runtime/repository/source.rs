//! Turn a pasted Git URL into a local checkout, using the credentials Git already has.
//!
//! Nothing here stores a secret. The clone runs the system `git` with prompts disabled, so a
//! configured credential helper, `gh` login, SSH agent or key works unchanged and everything
//! else fails fast as [`Failure::SignIn`]. The caller then asks for a personal access token,
//! which serves that one clone and may be handed to Git's own credential helper to keep.
use base64::Engine;
use horizon_cloud::Cancellation;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

/// A repository named by a URL, shorthand or browser address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Remote {
    pub url: String,
    pub host: String,
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
        let scheme = if scheme == "http" { "https" } else { scheme };
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
        (host_of(hostport), path, format!("{scheme}://{at}{hostport}/"))
    } else if let Some((account, path)) = input.split_once(':')
        && let Some((user, host)) = account.split_once('@')
        && valid_name(user)
    {
        (host.to_owned(), path, format!("{account}:"))
    } else {
        let (first, rest) = input.split_once('/')?;
        if first.contains('.') && rest.contains('/') {
            (first.to_owned(), rest, format!("https://{first}/"))
        } else if !first.contains('.') && !rest.contains('/') {
            ("github.com".to_owned(), input, "https://github.com/".to_owned())
        } else {
            return None;
        }
    };
    let path = repository_path(&host, path);
    let name = path.rsplit('/').next()?.to_owned();
    if !valid_host(&host) || path.split('/').count() < 2 || !path.split('/').all(valid_segment) {
        return None;
    }
    Some(Remote {
        url: format!("{prefix}{path}.git"),
        host,
        name,
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

/// The folders a clone of `remote` can land in under `parent`, in the order they are tried:
/// the repository's name, then that name with `-2`, `-3` and so on.
fn candidates<'a>(parent: &'a Path, remote: &'a Remote) -> impl Iterator<Item = PathBuf> + 'a {
    (1..=1000).map(move |n| match n {
        1 => parent.join(&remote.name),
        n => parent.join(format!("{}-{n}", remote.name)),
    })
}

/// A folder under `parent` named for the repository that nothing occupies yet.
#[must_use]
pub fn destination(parent: &Path, remote: &Remote) -> PathBuf {
    candidates(parent, remote)
        .find(|path| !path.exists())
        .unwrap_or_else(|| parent.join(&remote.name))
}

/// The checkout of `remote` already under `parent`, so a second request reuses it: any of the
/// folders [`destination`] would have chosen for an earlier clone.
#[must_use]
pub fn existing(parent: &Path, remote: &Remote) -> Option<PathBuf> {
    candidates(parent, remote).filter(|path| path.is_dir()).find(|path| {
        git_output(path, &["config", "--get", "remote.origin.url"])
            .and_then(|origin| parse(origin.trim()))
            .is_some_and(|origin| origin.url == remote.url)
            && is_checkout(path)
    })
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

/// Asks the person's configured Git credential helper to keep `token` for the host it was pasted for.
/// Returns whether a helper is configured to receive it.
#[must_use]
pub fn remember(token: &Token) -> bool {
    let configured = git_output(
        Path::new("."),
        &["config", "--get-regexp", r"^credential\.(.*\.)?helper$"],
    )
    .is_some_and(|helpers| !helpers.trim().is_empty());
    let Ok(mut child) = Command::new("git")
        .args(["credential", "approve"])
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
    child.wait().is_ok_and(|status| status.success()) && configured
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
    #[error("{0}")]
    Other(String),
}

fn classify(remote: &Remote, stderr: &str) -> Failure {
    let text = stderr.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    let web = remote.url.starts_with("https://");
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
    } else if !web && (refused || has(&["permission denied (", "host key verification failed"])) {
        // A token cannot help over SSH: the key, or trust in the host, is what is missing.
        Failure::Other(format!(
            "Git could not sign in over SSH. Give your SSH key access to this repository and trust {} once with `ssh`, or paste its https link.",
            remote.host
        ))
    } else if has(&["not found", "does not exist", "returned error: 404"]) {
        Failure::NotFound
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

/// Git's latest progress line, shared with the UI while the clone runs.
pub type Progress = Arc<Mutex<String>>;

/// `git` set up never to prompt, and to carry `token` when there is one.
fn git(token: Option<&Token>) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=30"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(token) = token {
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", format!("http.{}.extraHeader", token.scope))
            .env("GIT_CONFIG_VALUE_0", token.header());
    }
    // Never ask questions over SSH, but keep a command the person has set up themselves.
    if std::env::var_os("GIT_SSH_COMMAND").is_none()
        && git_output(Path::new("."), &["config", "--get", "core.sshCommand"]).is_none_or(|set| set.trim().is_empty())
    {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes -o StrictHostKeyChecking=yes");
    }
    command
}

/// Whether `remote` can be read without signing in, before anything is cloned.
///
/// # Errors
/// [`Failure::SignIn`] when it is private, or when the host hides a missing one the same way.
pub fn probe(remote: &Remote, token: Option<&Token>) -> Result<(), Failure> {
    let output = git(token)
        .args(["ls-remote", "--", &remote.url, "HEAD"])
        .stdout(Stdio::null())
        .output()
        .map_err(|error| Failure::Other(format!("Cannot run git: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(classify(remote, &String::from_utf8_lossy(&output.stderr)))
    }
}

/// Clones `remote` into `destination` without ever prompting.
///
/// # Errors
/// Names why the clone failed; a partial checkout is removed.
pub fn clone(
    remote: &Remote,
    destination: &Path,
    token: Option<&Token>,
    cancel: &Cancellation,
    progress: &Progress,
) -> Result<(), Failure> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| Failure::Other(error.to_string()))?;
    }
    // Only a folder this call makes is this clone's to remove: creating it is the claim, and a
    // folder that is already there is refused rather than cloned into.
    match std::fs::create_dir(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Failure::Other(
                "That folder already exists. Choose another clone folder and continue again.".into(),
            ));
        }
        Err(error) => return Err(Failure::Other(error.to_string())),
    }
    let mut command = git(token);
    command
        .args(["clone", "--progress", "--", &remote.url])
        .arg(destination)
        .stdout(Stdio::null());
    let mut child = command.spawn().map_err(|error| {
        let _ = std::fs::remove_dir_all(destination);
        Failure::Other(format!("Cannot run git: {error}"))
    })?;
    let mut stderr = child.stderr.take();
    let sink = Arc::clone(progress);
    let reader = std::thread::spawn(move || {
        let (mut all, mut line, mut buffer) = (String::new(), String::new(), [0_u8; 512]);
        while let Some(read) = stderr
            .as_mut()
            .and_then(|pipe| pipe.read(&mut buffer).ok())
            .filter(|n| *n > 0)
        {
            for character in String::from_utf8_lossy(&buffer[..read]).chars() {
                if matches!(character, '\r' | '\n') {
                    if !line.trim().is_empty() {
                        line.trim()
                            .clone_into(&mut sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
                        all.push_str(&line);
                        all.push('\n');
                    }
                    line.clear();
                } else {
                    line.push(character);
                }
            }
        }
        all
    });
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = std::fs::remove_dir_all(destination);
            return Err(Failure::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = std::fs::remove_dir_all(destination);
                return Err(Failure::Other(error.to_string()));
            }
        }
    };
    let stderr = reader.join().unwrap_or_default();
    if status.success() {
        Ok(())
    } else {
        let _ = std::fs::remove_dir_all(destination);
        Err(classify(remote, &stderr))
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
mod tests {
    use super::*;

    fn remote(input: &str) -> Option<(String, String, String)> {
        parse(input).map(|remote| (remote.url, remote.host, remote.name))
    }

    #[test]
    fn reads_every_common_way_to_name_a_repository() {
        let expected = Some((
            "https://github.com/peters/horizon.git".to_owned(),
            "github.com".to_owned(),
            "horizon".to_owned(),
        ));
        for input in [
            "peters/horizon",
            "github.com/peters/horizon",
            "https://github.com/peters/horizon",
            "https://github.com/peters/horizon.git",
            "https://github.com/peters/horizon/tree/main/crates",
            " https://github.com/peters/horizon/  ",
        ] {
            assert_eq!(remote(input), expected, "{input}");
        }
        assert_eq!(
            remote("git@github.com:peters/horizon.git"),
            Some((
                "git@github.com:peters/horizon.git".into(),
                "github.com".into(),
                "horizon".into()
            ))
        );
    }

    #[test]
    fn keeps_gitlab_groups_and_drops_browser_suffixes() {
        assert_eq!(
            remote("https://gitlab.com/group/sub/app/-/tree/main"),
            Some((
                "https://gitlab.com/group/sub/app.git".into(),
                "gitlab.com".into(),
                "app".into()
            ))
        );
        assert_eq!(
            remote("gitlab.example.org/team/app").map(|(_, host, _)| host),
            Some("gitlab.example.org".into())
        );
    }

    #[test]
    fn leaves_paths_and_unsafe_input_alone() {
        for input in [
            "",
            "/home/me/horizon",
            "~/horizon",
            "./horizon",
            "-oProxyCommand=x/y",
            "horizon",
            "file:///tmp/repo",
            "ext::sh -c id",
            "https://github.com/peters",
            "owner/repo name",
        ] {
            assert_eq!(parse(input), None, "{input}");
        }
    }

    #[test]
    fn sorts_clone_failures_by_what_the_person_can_do() {
        let github = parse("github.com/demo-org/demo").unwrap();
        for (stderr, expected) in [
            (
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
                Failure::SignIn("github.com".into()),
            ),
            (
                "remote: Repository not found.\nfatal: repository 'x' not found",
                Failure::NotFound,
            ),
            (
                "fatal: unable to access 'x': Could not resolve host: github.com",
                Failure::Network,
            ),
            (
                "fatal: could not create work tree dir 'x': Permission denied",
                Failure::Other("fatal: could not create work tree dir 'x': Permission denied".into()),
            ),
            ("fatal: something odd\n", Failure::Other("fatal: something odd".into())),
        ] {
            assert_eq!(classify(&github, stderr), expected, "{stderr}");
        }
    }

    #[test]
    fn ssh_failures_never_ask_for_a_token() {
        let ssh = parse("git@github.com:demo-org/demo").unwrap();
        for stderr in [
            "git@github.com: Permission denied (publickey).",
            "Host key verification failed.",
            "fatal: Authentication failed for 'ssh://git@github.com/demo-org/demo.git'",
        ] {
            assert!(
                matches!(classify(&ssh, stderr), Failure::Other(text) if text.contains("over SSH")),
                "{stderr}"
            );
        }
    }

    #[test]
    fn a_token_or_password_in_the_address_never_reaches_git() {
        let remote = parse("https://user:ghp_secret@github.com/demo-org/demo").unwrap();
        assert_eq!(remote.url, "https://github.com/demo-org/demo.git");
        let ssh = parse("ssh://git:secret@github.com/demo-org/demo").unwrap();
        assert_eq!(ssh.url, "ssh://git@github.com/demo-org/demo.git");
        assert!(!format!("{remote:?}{ssh:?}").contains("secret"));
        assert_eq!(parse("ssh://-oProxyCommand=a.b/demo-org/demo"), None);
        assert_eq!(parse("https://-bad.example.org/demo-org/demo"), None);
        assert_eq!(parse("-o@github.com:demo-org/demo"), None);
    }

    #[test]
    fn a_name_that_could_leave_the_clone_folder_is_refused() {
        for input in [
            r"github.com/demo-org/..\\victim",
            r"github.com/demo-org/C:\\victim",
            "github.com/demo-org/..",
            "https://github.com/demo-org/a%2Fb",
            "https://gitlab.com/group/../demo",
        ] {
            assert_eq!(parse(input), None, "{input}");
        }
        assert!(
            parse("github.com/demo-org/.github").is_some(),
            "a leading dot is a real name"
        );
    }

    #[test]
    fn a_token_travels_as_a_basic_header_for_its_host_only() {
        let github_remote = parse("github.com/demo-org/demo").unwrap();
        let github = Token::new(&github_remote, " ghp_demo \n").unwrap();
        assert_eq!(github.scope, "https://github.com/");
        assert_eq!(
            github.header(),
            format!(
                "Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD.encode("x-access-token:ghp_demo")
            )
        );
        let gitlab = parse("https://gitlab.example.org:8443/group/demo").unwrap();
        assert_eq!(
            Token::new(&gitlab, "glpat_demo").unwrap().scope,
            "https://gitlab.example.org:8443/"
        );
        assert!(Token::new(&gitlab, "with space").is_none());
        assert!(Token::new(&gitlab, "  ").is_none());
        let ssh = parse("git@github.com:demo-org/demo").unwrap();
        assert!(Token::new(&ssh, "ghp_demo").is_none(), "a token never applies over SSH");
    }

    #[test]
    fn picks_a_free_folder_named_for_the_repository() {
        let temp = tempfile::tempdir().unwrap();
        let remote = parse("peters/horizon").unwrap();
        assert_eq!(destination(temp.path(), &remote), temp.path().join("horizon"));
        std::fs::create_dir(temp.path().join("horizon")).unwrap();
        assert_eq!(destination(temp.path(), &remote), temp.path().join("horizon-2"));
        assert_eq!(default_parent(temp.path()), temp.path().join("Horizon"));
        std::fs::create_dir(temp.path().join("code")).unwrap();
        assert_eq!(default_parent(temp.path()), temp.path().join("code"));
    }

    #[test]
    fn clones_a_local_repository_and_recognises_the_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin");
        let git = |dir: &Path, args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        std::fs::create_dir(&origin).unwrap();
        git(&origin, &["init", "-q"]);
        git(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let remote = Remote {
            url: origin.to_string_lossy().into_owned(),
            host: "example.com".into(),
            name: "origin".into(),
        };
        let target = temp.path().join("nested/clone");
        assert!(!is_checkout(&target));
        assert_eq!(probe(&remote, None), Ok(()));
        let missing = Remote {
            url: temp.path().join("missing").to_string_lossy().into_owned(),
            ..remote.clone()
        };
        assert!(probe(&missing, None).is_err());
        clone(&remote, &target, None, &Cancellation::default(), &Progress::default()).unwrap();
        assert!(is_checkout(&target));
    }

    #[test]
    fn a_cancelled_or_failed_clone_removes_only_what_it_made() {
        let temp = tempfile::tempdir().unwrap();
        let remote = Remote {
            url: temp.path().join("missing").to_string_lossy().into_owned(),
            host: "example.com".into(),
            name: "missing".into(),
        };
        let cancelled = Cancellation::default();
        cancelled.cancel();
        let fresh = temp.path().join("fresh");
        assert_eq!(
            clone(&remote, &fresh, None, &cancelled, &Progress::default()),
            Err(Failure::Cancelled)
        );
        assert!(!fresh.exists(), "a folder the clone made is removed");
        let theirs = temp.path().join("theirs");
        std::fs::create_dir(&theirs).unwrap();
        std::fs::write(theirs.join("notes.txt"), "keep").unwrap();
        assert!(clone(&remote, &theirs, None, &Cancellation::default(), &Progress::default()).is_err());
        assert!(
            theirs.join("notes.txt").is_file(),
            "a folder that was already there stays"
        );
    }

    #[test]
    fn a_checkout_already_under_the_parent_is_recognised_for_its_own_link_only() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
                .args(args)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        };
        git(&origin, &["init", "-q"]);
        git(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let remote = parse("github.com/demo-org/demo").unwrap();
        let checkout = temp.path().join("demo");
        git(temp.path(), &["clone", "-q", origin.to_str().unwrap(), "demo"]);
        assert_eq!(existing(temp.path(), &remote), None, "another origin is not this link");
        git(&checkout, &["remote", "set-url", "origin", &remote.url]);
        assert_eq!(existing(temp.path(), &remote), Some(checkout.clone()));
        // A second copy made beside an occupied name is found too.
        let beside = temp.path().join("demo-2");
        git(temp.path(), &["clone", "-q", origin.to_str().unwrap(), "demo-2"]);
        git(
            &beside,
            &["remote", "set-url", "origin", "https://github.com/demo-org/other.git"],
        );
        git(
            &checkout,
            &["remote", "set-url", "origin", "https://github.com/demo-org/other.git"],
        );
        git(&beside, &["remote", "set-url", "origin", &remote.url]);
        assert_eq!(existing(temp.path(), &remote), Some(beside.clone()));
        // A gap before it (the first name taken by a file) does not hide it.
        std::fs::remove_dir_all(&checkout).unwrap();
        std::fs::write(&checkout, "not a folder").unwrap();
        assert_eq!(existing(temp.path(), &remote), Some(beside));
    }
}
