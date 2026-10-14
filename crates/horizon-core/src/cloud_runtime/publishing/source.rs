//! Credential-free source identity used only for package naming.
use super::{Destination, Error, Result};
use crate::cloud_runtime::command::Runner;
use std::{path::Path, process::Command, time::Duration};

pub(super) fn automatic(login: &str, repository: &Path, name: &str, runner: &Runner<'_>) -> Result<Destination> {
    runner.cancel.check()?;
    let mut command = Command::new("git");
    command
        .args(["-C"])
        .arg(repository)
        .args(["config", "--get", "remote.origin.url"]);
    // A remote can contain credentials. Never send this command's output to a card or log.
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
        .and_then(|origin| crate::cloud_runtime::repository::source::parse(origin.trim()))
        .map_or_else(
            || repository.to_string_lossy().into_owned(),
            |remote| {
                // A self-hosted service on another port is another source identity.
                let authority = remote
                    .url
                    .split_once("://")
                    .and_then(|(_, rest)| rest.split('/').next())
                    .map_or(remote.host.as_str(), |authority| {
                        authority.rsplit('@').next().unwrap_or(authority)
                    });
                format!("{authority}/{}/{}", remote.owner, remote.name)
            },
        );
    Destination::automatic_github(login, &source, name)
        .map_err(|_| Error::Invalid("Could not select a personal image destination"))
}

#[cfg(test)]
mod tests;
