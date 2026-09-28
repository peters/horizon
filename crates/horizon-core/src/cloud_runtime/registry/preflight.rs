//! Check the selected logins before building, without publishing a probe image.
mod acr;
use super::{Auth, Error, Material, Prepared, Result, Runner};
use crate::cloud_runtime::{Event, progress::Progress};
use std::{process::Command, time::Duration};

#[derive(Clone, Copy)]
enum Purpose {
    Pull,
    Publish,
}

impl Purpose {
    fn failure(self) -> Error {
        Error::Invalid(match self {
            Self::Pull => {
                "Worker pull credential preflight failed; check the saved login, repository permissions and registry connectivity before retrying"
            }
            Self::Publish => {
                "Publishing credential preflight failed; check the saved login, repository push permissions and registry connectivity before retrying"
            }
        })
    }

    fn actions(self) -> &'static [&'static str] {
        match self {
            Self::Pull => &["pull"],
            Self::Publish => &["pull", "push"],
        }
    }
}

impl Prepared {
    /// Checks the saved logins before image work. ACR also proves the requested
    /// repository actions through a fresh token grant. This is not immutable-image
    /// validation and never authorizes transferring a credential to the provider.
    /// # Errors
    /// Refuses authentication/scope failures, unreachable registries and cancellation.
    pub fn preflight(&self, runner: &Runner<'_>) -> Result<()> {
        self.preflight_with(runner, |auth, material, purpose| {
            let result = login(
                auth,
                material,
                &self.binding.repository,
                runner,
                self.docker_host.as_deref(),
                Command::new("docker"),
            )
            .and_then(|()| acr::verify(auth, material, &self.binding.repository, purpose, runner.cancel));
            runner.cancel.check()?;
            result.map_err(|_| purpose.failure())
        })
    }

    fn preflight_with(
        &self,
        runner: &Runner<'_>,
        mut check: impl FnMut(&Auth, &Material, Purpose) -> Result<()>,
    ) -> Result<()> {
        runner.cancel.check()?;
        (runner.emit)(Event::Progress(Progress::activity("Checking worker pull credentials")));
        self.binding.pull.check_expiry()?;
        self.pull.verify_scope(&self.binding.repository, runner.cancel)?;
        check(&self.binding.pull, &self.pull, Purpose::Pull)?;
        if let (Some(auth), Some(material)) = (&self.binding.publish, &self.publish) {
            (runner.emit)(Event::Progress(Progress::activity("Checking publishing credentials")));
            auth.check_expiry()?;
            check(auth, material, Purpose::Publish)?;
        }
        runner.cancel.check()?;
        Ok(())
    }
}

fn login(
    auth: &Auth,
    material: &Material,
    repository: &str,
    runner: &Runner<'_>,
    docker_host: Option<&str>,
    mut command: Command,
) -> Result<()> {
    let (host, _) = repository
        .split_once('/')
        .ok_or(Error::Invalid("Invalid registry repository"))?;
    // A fresh config forces the provided login and cannot overwrite the config
    // used by the build or worker probe with a returned identity token.
    let config = tempfile::tempdir()?;
    // Docker auto-selects an OS credential helper when the config has no auth
    // entries. An empty entry selects its file store without supplying a cached
    // login, keeping both secrets and successful-login writes task-local.
    std::fs::write(
        config.path().join("config.json"),
        serde_json::json!({"auths": {host: {}}}).to_string(),
    )?;
    command
        .env_remove("DOCKER_AUTH_CONFIG")
        .arg("--config")
        .arg(config.path());
    if let Some(host) = docker_host {
        command.arg("--host").arg(host);
    }
    command.args(["login", "--username", &auth.username, "--password-stdin", host]);
    runner.private_exchange(&mut command, material.secret.as_bytes(), Duration::from_secs(30))?;
    Ok(())
}

#[cfg(test)]
mod tests;
