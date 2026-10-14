//! Credential-free Git origin identity used only for package naming.
use super::{Destination, Error, Result};
use crate::cloud_runtime::command::Runner;
use std::{fmt::Write as _, path::Path, process::Command, time::Duration};

const ORIGIN_INVALID: &str =
    "Use an unambiguous Git origin without query parameters, or choose an explicit image destination";

pub(super) fn automatic(login: &str, repository: &Path, name: &str, runner: &Runner<'_>) -> Result<Destination> {
    runner.cancel.check()?;
    let repository = repository.canonicalize()?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(&repository)
        .args(["config", "--local", "--get-all", "remote.origin.url"]);
    // A remote can contain credentials. Never send its output to a card or log.
    let quiet = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let origin = match quiet.run_parsed("repository image identity", &mut command, Duration::from_secs(10)) {
        Ok(origin) => Some(origin),
        Err(Error::Command(_)) => None,
        Err(error) => return Err(error),
    };
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

fn remote(origin: &str) -> Result<Option<String>> {
    if origin.len() > 4096 || origin.contains(char::is_control) || origin.contains('\u{fffd}') {
        return Err(Error::Invalid(ORIGIN_INVALID));
    }
    if origin.starts_with(['/', '\\', '.', '~']) || origin.is_empty() {
        return Ok(None);
    }
    let (scheme, authority, path, home) = if let Some((scheme, rest)) = origin.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https" | "ssh" | "git") {
            return Ok(None);
        }
        let (authority, path) = rest.split_once('/').ok_or(Error::Invalid(ORIGIN_INVALID))?;
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
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty() {
        return Err(Error::Invalid(ORIGIN_INVALID));
    }
    let server = server.to_ascii_lowercase();
    if home {
        let Some(user) = user.filter(|user| !user.is_empty()) else {
            return Ok(None);
        };
        // SCP paths are relative to the SSH user's home. Keep that scope without retaining userinfo.
        let hash = ring::digest::digest(&ring::digest::SHA256, user.as_bytes());
        let scope = hash.as_ref().iter().fold(String::with_capacity(64), |mut scope, byte| {
            let _ = write!(scope, "{byte:02x}");
            scope
        });
        return Ok(Some(format!("ssh-home://{server}/{scope}/{path}")));
    }
    Ok(Some(format!("{scheme}://{server}/{path}")))
}

fn scp(origin: &str) -> Option<(&str, &str)> {
    let host = origin.rsplit_once('@').map_or(origin, |(_, host)| host);
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
