//! What a proof checks and clears before it builds: that the daemon mounts this
//! computer's files, and that no earlier proof's container of the run is left.
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

    /// Checks that the daemon mounts this computer's files: a private folder made here
    /// holds a random token, and a container of Horizon's base reads it back. A daemon
    /// on another machine behind a forwarded port or socket mounts its own path of that
    /// name instead, and a proof there would test other files than this computer's.
    pub(super) fn check_mounts(&self) -> Result<(), String> {
        let folder = tempfile::Builder::new()
            .prefix("horizon-proof-mounts-")
            .rand_bytes(16)
            .tempdir()
            .map_err(|error| format!("cannot make a private folder: {error}"))?;
        let token = folder
            .path()
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        std::fs::write(folder.path().join("token"), &token).map_err(|error| error.to_string())?;
        let mut read = self.docker();
        // The proof's own container name, so Stop and the next proof's cleanup remove it.
        read.args(["run", "--rm", "--network", "none", "--name", &self.container])
            .arg("-v")
            .arg(format!("{}:/horizon-mounts:ro", folder.path().display()))
            .args(["--entrypoint", "cat", super::TOOLS, "/horizon-mounts/token"]);
        // Long enough for a first pull of Horizon's base, which the proof needs anyway.
        let read = self
            .capture(read, super::BUILD_TIMEOUT)
            .map_err(|error| format!("Docker cannot run a container that reads this computer's files: {error}"))?;
        if String::from_utf8_lossy(&read).trim() == token {
            Ok(())
        } else {
            Err("Docker here mounts another machine's files, not this computer's, so Horizon cannot prove the setup with it: use Docker running on this computer".to_owned())
        }
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
        remove.args(["rm", "--force"]).args(&earlier);
        self.capture(remove, PREFLIGHT_TIMEOUT)
            .map(|_| ())
            .map_err(|error| format!("Docker does not remove the containers of earlier proofs: {error}"))
    }
}
