//! Explicit non-secret provider configuration, independent of provider I/O and UI.

use crate::cloud_run::{
    interactive_worker::valid_worker_profile_name, local_docker::LocalDockerProfile, runpod::RunPodProfile,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Empty by default: no ambient daemon, context or profile is implicitly selected.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RemoteProviderConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub local_docker: Vec<LocalDockerProfile>,
    /// Non-secret placement only. The controller reads `RUNPOD_API_KEY` only on an explicit connection.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runpod: Vec<RunPodProfile>,
}

impl<'de> Deserialize<'de> for RemoteProviderConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(default)]
            local_docker: Vec<LocalDockerProfile>,
            #[serde(default)]
            runpod: Vec<RunPodProfile>,
        }

        // Parser errors can contain malformed values, even in a non-secret configuration block.
        Fields::deserialize(deserializer)
            .map(|fields| Self {
                local_docker: fields.local_docker,
                runpod: fields.runpod,
            })
            .map_err(|_| serde::de::Error::custom("invalid remote provider configuration"))
    }
}

impl RemoteProviderConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.local_docker.is_empty() && self.runpod.is_empty()
    }

    /// Validate explicit names and local endpoints without provider or credential access.
    /// Profile names match saved worker targets exactly, including letter case.
    /// # Errors
    /// Rejects malformed profiles and duplicate names without echoing configured values.
    pub fn validate(&self) -> Result<(), RemoteProviderConfigError> {
        let mut names = HashSet::new();
        for (index, profile) in self.local_docker.iter().enumerate() {
            profile
                .validate()
                .map_err(|_| RemoteProviderConfigError::InvalidLocalProfile { index })?;
            if !names.insert(profile.name.as_str()) {
                return Err(RemoteProviderConfigError::DuplicateLocalProfile { index });
            }
        }
        names.clear();
        for (index, profile) in self.runpod.iter().enumerate() {
            if !valid_worker_profile_name(&profile.name) {
                return Err(RemoteProviderConfigError::InvalidRunPodProfile { index });
            }
            if !names.insert(profile.name.as_str()) {
                return Err(RemoteProviderConfigError::DuplicateRunPodProfile { index });
            }
        }
        Ok(())
    }

    /// Resolve only an explicitly named profile. No default or environment fallback exists.
    /// # Errors
    /// Rejects invalid configuration or a name that has not been configured exactly.
    pub fn local_docker_profile(&self, name: &str) -> Result<&LocalDockerProfile, RemoteProviderConfigError> {
        self.validate()?;
        self.local_docker
            .iter()
            .find(|profile| profile.name == name)
            .ok_or(RemoteProviderConfigError::UnconfiguredLocalProfile)
    }

    /// Resolve an explicit `RunPod` name without credential or provider access.
    /// Placement is additionally validated against the saved target before connection.
    /// # Errors
    /// Rejects invalid names, duplicates and absent configuration without echoing values.
    pub fn runpod_profile(&self, name: &str) -> Result<&RunPodProfile, RemoteProviderConfigError> {
        self.validate()?;
        self.runpod
            .iter()
            .find(|profile| profile.name == name)
            .ok_or(RemoteProviderConfigError::UnconfiguredRunPodProfile)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RemoteProviderConfigError {
    #[error("remote.local_docker[{index}] requires a valid name and an explicit local socket or named pipe")]
    InvalidLocalProfile { index: usize },
    #[error("remote.local_docker[{index}] repeats an earlier profile name")]
    DuplicateLocalProfile { index: usize },
    #[error("no local provider profile exactly matches this saved environment")]
    UnconfiguredLocalProfile,
    #[error("remote.runpod[{index}] requires a valid explicit profile name")]
    InvalidRunPodProfile { index: usize },
    #[error("remote.runpod[{index}] repeats an earlier profile name")]
    DuplicateRunPodProfile { index: usize },
    #[error("no RunPod profile exactly matches this saved environment")]
    UnconfiguredRunPodProfile,
}

#[cfg(test)]
mod tests;
