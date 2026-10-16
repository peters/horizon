//! What a proof checks and clears before it builds: that no earlier proof's container of
//! the run is left, and the plan of the CI it runs.
use std::time::Duration;

/// The longest a docker command may take that only prepares a proof, such as removing
/// an earlier proof's container.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_mins(2);

impl super::Job {
    /// The run's throwaway Docker configuration, made once: only Horizon's own ghcr.io
    /// login, never this computer's credential helpers, so a private ghcr.io base pulls
    /// with it and every other image anonymously. Also whether it holds that login.
    pub(super) fn docker_config(&self) -> Result<(&std::path::Path, bool), String> {
        if self.docker_config.get().is_none() {
            let horizon_docker = horizon_core::HorizonHome::resolve().root().join("cloud").join("docker");
            let made = horizon_core::cloud_runtime::github::publish::build_config(&horizon_docker)
                .map_err(|error| error.to_string())?;
            let _ = self.docker_config.set(made);
        }
        self.docker_config
            .get()
            .map(|(config, signed_in)| (config.path(), *signed_in))
            .ok_or_else(|| "Horizon has no Docker configuration for the proof".to_owned())
    }

    /// Removes the containers of earlier proofs of this run's volume.
    pub(super) fn remove_earlier(&self) -> Result<(), String> {
        let mut list = self.docker();
        list.args(["ps", "--all", "--quiet", "--filter"])
            .arg(format!("name=^/{}-proof-", self.volume));
        let listed = self
            .capture(list, PREFLIGHT_TIMEOUT)
            .map_err(|error| format!("Docker does not list the containers of earlier proofs: {error}"))?;
        let earlier: Vec<String> = String::from_utf8_lossy(&listed)
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if earlier.is_empty() {
            return Ok(());
        }
        let mut remove = self.docker();
        remove.args(["rm", "--force", "--volumes"]).args(&earlier);
        self.capture(remove, PREFLIGHT_TIMEOUT)
            .map(|_| ())
            .map_err(|error| format!("Docker does not remove the containers of earlier proofs: {error}"))
    }

    /// Checks with the trusted localci, before the CI runs, that the proof keeps a step
    /// that runs a command in every job it selects.
    pub(super) fn check_plan(&self) -> Result<(), String> {
        // A proof with plain commands has no workflow to plan.
        if !self.spec.commands.is_empty() {
            return Ok(());
        }
        let mut launch = self.ci_container()?;
        launch
            .command
            .args(["--entrypoint", super::localci::IN_CONTAINER, &self.image])
            .args(self.spec.plan_args());
        let plan = self.capture_launch(launch, super::STEP_TIMEOUT)?;
        let (planned, left_out) = super::localci::check_plan(&plan)?;
        if !left_out.is_empty() {
            let line = format!("Not proved here: {}", left_out.join(", "));
            self.record(&mut None, line);
        }
        *self.planned.borrow_mut() = planned;
        Ok(())
    }
}
