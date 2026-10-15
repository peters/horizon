//! Credential-free Git origin identity used only for package naming.
use super::{Destination, Error, Result};
use crate::cloud_runtime::command::Runner;
use std::{fmt::Write as _, path::Path, process::Command, time::Duration};

const ORIGIN_READ: &str =
    "Could not read this checkout's Git origin; repair its Git configuration before choosing an automatic image";
const ORIGIN_INVALID: &str =
    "Use an unambiguous Git origin without query parameters, or choose an explicit image destination";

pub(super) fn automatic(login: &str, repository: &Path, name: &str, runner: &Runner<'_>) -> Result<Destination> {
    runner.cancel.check()?;
    let repository = repository.canonicalize()?;
    // A remote can contain credentials. Never send its output to a card or log.
    let quiet = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let origin = read_origin(&repository, &quiet)?;
    let source = origin
        .as_deref()
        .map(|origin| {
            origin
                .strip_suffix("\r\n")
                .or_else(|| origin.strip_suffix('\n'))
                .unwrap_or(origin)
        })
        .map(remote)
        .transpose()?
        .flatten();
    let source = match source {
        Some(source) => source,
        None => repository
            .to_str()
            .ok_or(Error::Invalid(
                "Automatic images need a UTF-8 checkout path or an explicit image destination",
            ))?
            .to_owned(),
    };
    Destination::automatic_github(login, &source, name)
        .map_err(|_| Error::Invalid("Could not select a personal image destination"))
}

fn read_origin(repository: &Path, runner: &Runner<'_>) -> Result<Option<String>> {
    let run = |command: &mut Command| {
        runner
            .run_parsed("repository image identity", command, Duration::from_secs(10))
            .map_err(|error| match error {
                Error::Command(_) => Error::Invalid(ORIGIN_READ),
                other => other,
            })
    };
    let mut probe = git_config(repository);
    probe.args(["--show-origin", "--null", "--get", "--default", "", "remote.origin.url"]);
    let found = run(&mut probe)?;
    // Git labels only the absent key's --default as command-line data. A real value,
    // including an empty one, has a file origin; the second read must then succeed.
    if found == "command line:\0\0" {
        return Ok(None);
    }
    if !found.starts_with("file:") {
        return Err(Error::Invalid(ORIGIN_READ));
    }
    let mut command = git_config(repository);
    command.args(["--get-all", "remote.origin.url"]);
    run(&mut command).map(Some)
}

fn git_config(repository: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env("LC_ALL", "C")
        .arg("-C")
        .arg(repository)
        .args(["config", "--local", "--includes"]);
    command
}

fn remote(origin: &str) -> Result<Option<String>> {
    if origin.len() > 4096 || origin.contains(char::is_control) || origin.contains('\u{fffd}') {
        return Err(Error::Invalid(ORIGIN_INVALID));
    }
    if origin.starts_with(['/', '\\', '.', '~']) || origin.is_empty() {
        return Ok(None);
    }
    let (scheme, authority, path, relative) = if let Some((scheme, rest)) = origin.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "ssh" | "git") {
            return Ok(None);
        }
        let (authority, path) = rest.split_once('/').ok_or(Error::Invalid(ORIGIN_INVALID))?;
        if authority.contains(['?', '#', '\\']) {
            return Err(Error::Invalid(ORIGIN_INVALID));
        }
        (scheme, authority, path, false)
    } else if let Some((authority, path)) = scp(origin) {
        if authority.len() == 1 && authority.bytes().all(|b| b.is_ascii_alphabetic()) && path.starts_with(['/', '\\']) {
            return Ok(None);
        }
        (
            "ssh".into(),
            authority,
            path.trim_start_matches('/'),
            !path.starts_with('/'),
        )
    } else {
        // A relative local origin is scoped to this checkout, never expanded as a browser shorthand.
        return Ok(None);
    };
    let (user, server) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(user, host)| (Some(user), host));
    let host = url::Url::parse(&format!("https://{server}/")).map_err(|_| Error::Invalid(ORIGIN_INVALID))?;
    if host.host_str().is_none()
        || !host.username().is_empty()
        || host.password().is_some()
        || host.query().is_some()
        || host.fragment().is_some()
        || host.path() != "/"
        || path.is_empty()
        || path.contains(['?', '#', '\\'])
    {
        return Err(Error::Invalid(ORIGIN_INVALID));
    }
    let decoded;
    let path = if scheme == "ssh" && origin.contains("://") {
        decoded = decode_ssh(path)?;
        decoded.as_str()
    } else {
        path
    };
    let decoded_user = if scheme == "ssh" && origin.contains("://") {
        user.map(|user| decode_ssh(user.split_once(':').map_or(user, |(user, _)| user)))
            .transpose()?
    } else {
        user.map(str::to_owned)
    };
    let (home, path) = ssh_home(&scheme, path, relative, origin.contains("://"), decoded_user.as_deref())?;
    if matches!(home, Home::Unknown) {
        return Ok(None);
    }
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        return Err(Error::Invalid(ORIGIN_INVALID));
    }
    let server = server.to_ascii_lowercase();
    if let Home::User(user) = home {
        // Home-relative paths retain their SSH user scope without retaining userinfo.
        let hash = ring::digest::digest(&ring::digest::SHA256, user.as_bytes());
        let scope = hash.as_ref().iter().fold(String::with_capacity(64), |mut scope, byte| {
            let _ = write!(scope, "{byte:02x}");
            scope
        });
        return Ok(Some(format!("ssh-home://{server}/{scope}/{path}")));
    }
    Ok(Some(format!("{scheme}://{server}/{path}")))
}

enum Home {
    Absolute,
    User(String),
    Unknown,
}

fn ssh_home<'a>(scheme: &str, path: &'a str, relative: bool, uri: bool, user: Option<&str>) -> Result<(Home, &'a str)> {
    if scheme != "ssh" {
        return Ok((Home::Absolute, path));
    }
    if let Some(home) = path.strip_prefix('~').filter(|_| relative || uri) {
        let (name, path) = home.split_once('/').ok_or(Error::Invalid(ORIGIN_INVALID))?;
        let user = if name.is_empty() { user } else { Some(name) };
        return Ok((home_user(user), path));
    }
    Ok((if relative { home_user(user) } else { Home::Absolute }, path))
}

fn home_user(user: Option<&str>) -> Home {
    user.filter(|user| !user.is_empty())
        .map_or(Home::Unknown, |user| Home::User(user.to_owned()))
}

fn decode_ssh(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = input.next().and_then(|byte| char::from(byte).to_digit(16));
            let low = input.next().and_then(|byte| char::from(byte).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(Error::Invalid(ORIGIN_INVALID));
            };
            bytes.push(u8::try_from(high * 16 + low).map_err(|_| Error::Invalid(ORIGIN_INVALID))?);
        } else {
            bytes.push(byte);
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| Error::Invalid(ORIGIN_INVALID))?;
    if decoded.contains(char::is_control) || decoded.contains(['?', '#', '\\']) {
        return Err(Error::Invalid(ORIGIN_INVALID));
    }
    Ok(decoded)
}

fn scp(origin: &str) -> Option<(&str, &str)> {
    let prefix = origin.split(':').next().unwrap_or(origin);
    let host = prefix.rfind('@').map_or(origin, |at| &origin[at + 1..]);
    let colon = if host.starts_with('[') {
        let end = host.find(']')?;
        let colon = origin.len() - host.len() + end + 1;
        (origin.as_bytes().get(colon) == Some(&b':')).then_some(colon)?
    } else {
        origin.find(':')?
    };
    let (authority, path) = origin.split_at(colon);
    if authority.contains(['/', '\\']) {
        return None;
    }
    Some((authority, &path[1..]))
}

#[cfg(test)]
mod tests;
