//! The Docker containers that run one operation's image contract checks.
use super::super::{Duration, Result, Runner};
use crate::cloud_runtime::{Error, new_id, worker_contract};
use horizon_cloud::{Cancellation, Capabilities};
use std::process::Command;

/// Names the operation a contract container checks, so a later check of that operation
/// finds the containers an interrupted one left.
const OPERATION_LABEL: &str = "horizon.contract.operation";

/// The contract checks of `operation_id`. Callers hold that operation's deployment lock, so
/// no other check of it runs and every container found belongs to an earlier, finished one.
pub(super) struct Checks<'a, D> {
    pub docker: D,
    pub runner: &'a Runner<'a>,
    pub operation_id: &'a str,
}

impl<D: Fn() -> Command> Checks<'_, D> {
    /// Runs the worker check of `image` in a container of its own and `validate`s its report.
    /// Removes the containers of earlier checks first and this check's own afterwards.
    pub(super) fn run<T>(
        &self,
        image: &str,
        capabilities: &Capabilities,
        git_auth: bool,
        validate: impl FnOnce(&str) -> Result<T>,
    ) -> Result<T> {
        self.remove()?;
        let result = self
            .attempt(image, capabilities, git_auth)
            .and_then(|output| validate(&output));
        // Killing a Docker client does not stop its daemon-owned container.
        super::finish_cleanup(result, self.remove(), self.runner.emit)
    }

    /// The name earlier Horizon versions gave every check of the operation.
    fn shared_name(&self) -> String {
        format!("horizon-contract-{}", self.operation_id)
    }

    /// A killed `docker create` can still hold its name in the daemon until the daemon has
    /// created the container. Until then no listing shows it and `rm` finds nothing, so a
    /// later check that reused the name would conflict with it. Each check takes a new name.
    fn attempt(&self, image: &str, capabilities: &Capabilities, git_auth: bool) -> Result<String> {
        let name = format!("{}-{}", self.shared_name(), new_id());
        let mut command = (self.docker)();
        command.args([
            "create",
            "--name",
            &name,
            "--label",
            &format!("{OPERATION_LABEL}={}", self.operation_id),
            "--network=none",
            "--entrypoint",
            "/usr/local/bin/horizon-worker-check",
            "--env",
            &worker_contract::environment(capabilities)?,
            image,
        ]);
        if git_auth {
            command.arg("--git-auth");
        }
        self.runner
            .run("worker image contract creation", &mut command, Duration::from_secs(30))?;
        self.runner.run(
            "worker image contract",
            (self.docker)().args(["start", "--attach", &name]),
            Duration::from_secs(60),
        )
    }

    /// Removes every listed container of the operation, including one under the shared
    /// name of an earlier Horizon version. Cleanup runs even after a cancellation.
    fn remove(&self) -> Result<()> {
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let filters = [
            format!("label={OPERATION_LABEL}={}", self.operation_id),
            format!("name=^/{}$", self.shared_name()),
        ];
        let listed = |step| -> Result<Vec<String>> {
            let mut containers = Vec::new();
            for filter in &filters {
                let output = runner.run(
                    step,
                    (self.docker)().args(["container", "ls", "--all", "--quiet", "--filter", filter]),
                    Duration::from_secs(15),
                )?;
                containers.extend(output.split_whitespace().map(str::to_owned));
            }
            Ok(containers)
        };
        let containers = listed("worker contract cleanup listing")?;
        if containers.is_empty() {
            return Ok(());
        }
        // A container already gone is clean. Verify absence independently of rm's exit code.
        let _ = runner.run(
            "worker contract cleanup",
            (self.docker)()
                .args(["container", "rm", "--force", "--volumes"])
                .args(&containers),
            Duration::from_secs(15),
        );
        // A killed create the daemon finishes meanwhile adds a container this cleanup did not
        // try to remove. That is no failure: the next check of the operation removes it.
        let remaining = listed("worker contract cleanup verification")?;
        if remaining.iter().any(|container| containers.contains(container)) {
            Err(Error::Invalid("Worker image contract container cleanup failed"))
        } else {
            Ok(())
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
