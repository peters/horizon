mod container;
use super::{Error, Event, Images, Result};
use crate::cloud_runtime::{WorkerContract, git_auth, repository::launch::quick_start, siblings, worker_contract};
use container::Checks;
use horizon_cloud::{CloudError, Profile};

impl Images<'_> {
    pub(super) fn validate(&self, image: &str, operation_id: &str, profile: &Profile) -> Result<()> {
        self.validate_contract(image, operation_id, profile, false).map(|_| ())
    }

    /// The optional features the image reports, such as pinned submodule history.
    /// # Errors
    /// Checks the selected runtime and optional Git binding before allocating compute.
    pub fn validate_contract(
        &self,
        image: &str,
        operation_id: &str,
        profile: &Profile,
        git_auth: bool,
    ) -> Result<WorkerContract> {
        self.check(image, operation_id, profile, git_auth, None)
    }

    /// # Errors
    /// As [`Self::validate_contract`] for an image with same-worker siblings, which must also
    /// lay out sibling checkouts and, with `grants`, accept version 2 Git grants.
    pub fn validate_siblings_contract(
        &self,
        image: &str,
        operation_id: &str,
        profile: &Profile,
        grants: bool,
    ) -> Result<WorkerContract> {
        self.check(image, operation_id, profile, grants, Some(grants))
    }

    fn check(
        &self,
        image: &str,
        operation_id: &str,
        profile: &Profile,
        git_auth: bool,
        sibling_grants: Option<bool>,
    ) -> Result<WorkerContract> {
        let capabilities = &profile.capabilities;
        if !horizon_cloud::valid_id(operation_id) {
            return Err(Error::Invalid("Invalid image operation identity"));
        }
        if let Some(report) = quick_start::trusted_contract(image, capabilities) {
            return validated(report, profile, git_auth, sibling_grants);
        }
        super::require_docker()?;
        Checks {
            docker: || self.docker(),
            runner: self.runner,
            operation_id,
        }
        .run(image, capabilities, git_auth, |output| {
            validated(output, profile, git_auth, sibling_grants)
        })
    }
}

/// The features `output`, a worker check report, promises once it satisfies `profile`.
fn validated(output: &str, profile: &Profile, git_auth: bool, sibling_grants: Option<bool>) -> Result<WorkerContract> {
    worker_contract::validate(
        output,
        &profile.capabilities,
        git_auth,
        profile.idle_stop_minutes.is_some(),
    )?;
    worker_contract::validate_idle_report(output, stopped_by_horizon(profile))?;
    sibling_grants.map_or(Ok(()), |grants| validate_siblings(output, grants))?;
    Ok(WorkerContract::reported(output))
}

fn validate_siblings(output: &str, grants: bool) -> Result<()> {
    let reports = |marker| worker_contract::reports(output, marker);
    if !reports(siblings::CONTRACT) {
        return Err(Error::Invalid(
            "Worker image does not support same-worker siblings; rebuild with the current worker bootstrap",
        ));
    }
    if grants && !reports(git_auth::GRANTS_CONTRACT) {
        return Err(Error::Invalid(
            "Worker image does not support per-repository Git credentials; rebuild with the current worker bootstrap",
        ));
    }
    Ok(())
}

/// The outcome of `result` after its cleanup. A cancellation stays the reported outcome, as
/// callers recognize it, and a cleanup failure beside it is only emitted; otherwise as
/// [`finish_contract`].
pub(super) fn finish_cleanup<T>(result: Result<T>, cleanup: Result<()>, emit: &dyn Fn(Event)) -> Result<T> {
    if let (Err(Error::Provider(CloudError::Cancelled)), Err(failed)) = (&result, &cleanup) {
        emit(Event::Output(failed.to_string()));
        return result;
    }
    finish_contract(result, cleanup)
}

/// The outcome of `result`, keeping a cleanup failure visible beside it.
fn finish_contract<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (result, cleanup) {
        (Err(primary), Err(cleanup)) => Err(Error::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(value), Ok(())) => Ok(value),
    }
}

/// Whether Horizon, not the worker, stops this profile's cloud when it is idle.
fn stopped_by_horizon(profile: &Profile) -> bool {
    profile.idle_stop_minutes.is_some()
        && horizon_cloud::provider::by_id(&profile.provider)
            .is_some_and(|provider| provider.idle_stop == horizon_cloud::provider::IdleStop::Horizon)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_runtime::{Event, command::Runner};
    use horizon_cloud::Cancellation;

    #[test]
    fn contract_rejection_and_cleanup_failure_remain_distinct() {
        let error = finish_contract::<()>(
            Err(Error::Invalid("missing agent")),
            Err(Error::Invalid("cleanup pending")),
        )
        .unwrap_err();
        assert!(matches!(error, Error::Cleanup { .. }));
        assert_eq!(error.to_string(), "missing agent; cleanup also failed: cleanup pending");
        assert!(finish_contract(Ok(()), Err(Error::Invalid("cleanup pending"))).is_err());
        assert!(finish_contract::<()>(Err(Error::Invalid("missing agent")), Ok(())).is_err());
        assert!(finish_contract(Ok(()), Ok(())).is_ok());
    }

    #[test]
    fn cancellation_stays_the_outcome_when_cleanup_also_fails() {
        let output = std::cell::RefCell::new(Vec::new());
        let emit = |event| {
            if let Event::Output(line) = event {
                output.borrow_mut().push(line);
            }
        };
        let cancelled = finish_cleanup::<()>(
            Err(CloudError::Cancelled.into()),
            Err(Error::Invalid("cleanup pending")),
            &emit,
        );
        assert!(matches!(cancelled, Err(Error::Provider(CloudError::Cancelled))));
        assert_eq!(*output.borrow(), ["cleanup pending"]);
        let failed = finish_cleanup::<()>(
            Err(Error::Invalid("missing agent")),
            Err(Error::Invalid("cleanup pending")),
            &emit,
        );
        assert!(matches!(failed, Err(Error::Cleanup { .. })));
        assert!(finish_cleanup::<()>(Err(CloudError::Cancelled.into()), Ok(()), &emit).is_err());
        assert_eq!(output.borrow().len(), 1);
    }

    #[test]
    fn sibling_images_must_report_sibling_checkouts_and_version_2_grants_when_sent() {
        let rejected = |output: &str, grants| match validate_siblings(output, grants) {
            Err(Error::Invalid(message)) => message,
            other => panic!("unexpected {other:?}"),
        };
        assert!(rejected("horizon-source-contract=1", false).contains("same-worker siblings"));
        assert!(
            rejected("horizon-siblings-contract=1\nhorizon-git-auth-contract=1", true).contains("per-repository Git")
        );
        assert!(rejected("prefix horizon-siblings-contract=1", false).contains("siblings"));
        assert!(validate_siblings("horizon-siblings-contract=1", false).is_ok());
        assert!(
            validate_siblings(
                "horizon-siblings-contract=1\nhorizon-git-auth-contract=1\nhorizon-git-auth-contract=2",
                true
            )
            .is_ok()
        );
    }

    #[test]
    #[ignore = "requires a task-owned Docker daemon and HORIZON_TEST_CONTRACT_IMAGE whose check prints contract-running then sleeps"]
    fn cancellation_removes_the_daemon_container_and_recovers_previous_attempt() {
        let host = std::env::var("HORIZON_TEST_DOCKER_HOST").unwrap();
        let image = std::env::var("HORIZON_TEST_CONTRACT_IMAGE").unwrap();
        let config = tempfile::tempdir().unwrap();
        let cancel = Cancellation::default();
        let operation = crate::cloud_runtime::new_id();
        // An earlier Horizon version gave every check of the operation this name.
        let name = format!("horizon-contract-{operation}");
        let volumes = std::sync::Mutex::new(Vec::new());
        let emit = |event| {
            if matches!(event, Event::Output(line) if line == "contract-running") {
                for container in contract_containers(&host, &format!("label=horizon.contract.operation={operation}")) {
                    volumes.lock().unwrap().extend(container_volumes(&host, &container));
                }
                cancel.cancel();
            }
        };
        let runner = Runner {
            cancel: &cancel,
            emit: &emit,
            secrets: Vec::new(),
        };
        let images = Images {
            isolated_registry: false,
            docker_host: Some(&host),
            docker_config: config.path(),
            runner: &runner,
            publisher: None,
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
        let profile = horizon_cloud::CloudConfig::parse(horizon_cloud::EXAMPLE)
            .unwrap()
            .profiles
            .remove("image-only")
            .unwrap();
        assert!(images.validate(&image, &operation, &profile).is_err());
        assert!(cancel.is_cancelled());
        for filter in [
            format!("name=^/{name}$"),
            format!("label=horizon.contract.operation={operation}"),
        ] {
            assert_eq!(contract_containers(&host, &filter), Vec::<String>::new());
        }
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

    fn contract_containers(host: &str, filter: &str) -> Vec<String> {
        let output = std::process::Command::new("docker")
            .args([
                "--host",
                host,
                "container",
                "ls",
                "--all",
                "--quiet",
                "--filter",
                filter,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
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
