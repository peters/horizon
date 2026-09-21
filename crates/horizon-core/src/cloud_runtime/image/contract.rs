use super::{Duration, Error, Images, Result, Runner};
use crate::cloud_runtime::worker_contract;
use horizon_cloud::{Cancellation, Capabilities};

impl Images<'_> {
    pub(super) fn validate(&self, image: &str, operation_id: &str, capabilities: &Capabilities) -> Result<()> {
        self.validate_contract(image, operation_id, capabilities, false)
    }

    /// # Errors
    /// Checks the selected runtime and optional Git binding before allocating compute.
    pub fn validate_contract(
        &self,
        image: &str,
        operation_id: &str,
        capabilities: &Capabilities,
        git_auth: bool,
    ) -> Result<()> {
        if !horizon_cloud::valid_id(operation_id) {
            return Err(Error::Invalid("Invalid image operation identity"));
        }
        let name = format!("horizon-contract-{operation_id}");
        // The durable operation identity also recovers an interrupted previous check.
        self.remove_contract(&name)?;
        let result = self.run_contract(image, &name, capabilities, git_auth);
        // Killing a Docker client does not stop its daemon-owned container.
        self.remove_contract(&name)?;
        let output = result?;
        worker_contract::validate(&output, capabilities, git_auth)
    }

    fn run_contract(&self, image: &str, name: &str, capabilities: &Capabilities, git_auth: bool) -> Result<String> {
        let mut command = self.docker();
        command.args([
            "create",
            "--name",
            name,
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
            self.docker().args(["start", "--attach", name]),
            Duration::from_secs(60),
        )
    }

    fn remove_contract(&self, name: &str) -> Result<()> {
        let cancel = Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        // A missing container is already clean. Verify absence independently of rm's exit code.
        let _ = runner.run(
            "worker contract cleanup",
            self.docker().args(["container", "rm", "--force", "--volumes", name]),
            Duration::from_secs(15),
        );
        let remaining = runner.run(
            "worker contract cleanup verification",
            self.docker().args([
                "container",
                "ls",
                "--all",
                "--quiet",
                "--filter",
                &format!("name=^/{name}$"),
            ]),
            Duration::from_secs(15),
        )?;
        if !remaining.trim().is_empty() {
            return Err(Error::Invalid("Worker image contract container cleanup failed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_runtime::Event;

    #[test]
    #[ignore = "requires a task-owned Docker daemon and HORIZON_TEST_CONTRACT_IMAGE whose check prints contract-running then sleeps"]
    fn cancellation_removes_the_daemon_container_and_recovers_previous_attempt() {
        let host = std::env::var("HORIZON_TEST_DOCKER_HOST").unwrap();
        let image = std::env::var("HORIZON_TEST_CONTRACT_IMAGE").unwrap();
        let config = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let operation = crate::cloud_runtime::new_id();
        let name = format!("horizon-contract-{operation}");
        let volumes = std::sync::Mutex::new(Vec::new());
        let emit = |event| {
            if matches!(event, Event::Output(line) if line == "contract-running") {
                volumes.lock().unwrap().extend(container_volumes(&host, &name));
                cancel.cancel();
            }
        };
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        };
        let images = Images {
            docker_host: Some(&host),
            docker_config: config.path(),
            runner: &runner,
        };
        assert!(
            images
                .docker()
                .args([
                    "run",
                    "--detach",
                    "--name",
                    &name,
                    "--network=none",
                    "--entrypoint",
                    "/usr/local/bin/horizon-worker-check",
                    &image
                ])
                .status()
                .unwrap()
                .success()
        );
        volumes.lock().unwrap().extend(container_volumes(&host, &name));
        assert!(images.validate(&image, &operation, &Capabilities::default()).is_err());
        assert!(cancel.is_cancelled());
        let output = images
            .docker()
            .args([
                "container",
                "ls",
                "--all",
                "--quiet",
                "--filter",
                &format!("name=^/{name}$"),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        let volumes = volumes.lock().unwrap().clone();
        assert!(volumes.len() >= 2, "fixture image must declare an anonymous VOLUME");
        for volume in volumes {
            assert!(
                !images
                    .docker()
                    .args(["volume", "inspect", &volume])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
    }

    fn container_volumes(host: &str, name: &str) -> Vec<String> {
        let output = std::process::Command::new("docker")
            .args(["--host", host, "inspect", "--format", "{{json .Mounts}}", name])
            .output()
            .unwrap();
        assert!(output.status.success());
        let mounts: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        mounts
            .iter()
            .filter(|mount| mount["Type"] == "volume")
            .map(|mount| mount["Name"].as_str().unwrap().to_owned())
            .collect()
    }
}
