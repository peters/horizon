//! Portable repository launch intent. Secrets and account bindings are not accepted.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CloudConfig {
    pub version: u32,
    pub default: String,
    pub profiles: BTreeMap<String, Profile>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub provider: String,
    pub image: String,
    pub cpu: u16,
    pub memory_gb: u16,
    #[serde(default)]
    pub gpu: bool,
    #[serde(default)]
    pub build: Option<Build>,
    #[serde(default)]
    pub storage: Storage,
    #[serde(default)]
    pub bootstrap: Bootstrap,
    #[serde(default)]
    pub capabilities: crate::Capabilities,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub context: String,
    pub dockerfile: String,
    #[serde(default = "default_platform")]
    pub platform: String,
}
fn default_platform() -> String {
    "linux/amd64".into()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Storage {
    pub container_gb: u16,
    pub volume_gb: u16,
}
impl Default for Storage {
    fn default() -> Self {
        Self {
            container_gb: 20,
            volume_gb: 20,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Bootstrap {
    pub readiness_seconds: u16,
    pub contract_version: u16,
}
impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            readiness_seconds: 600,
            contract_version: 1,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    // Parser errors may embed input values (including mistakenly entered secrets).
    #[error("Invalid cloud.yml syntax or unsupported fields")]
    Yaml,
    #[error("{0}")]
    Invalid(&'static str),
}

impl CloudConfig {
    /// # Errors
    /// Rejects unknown fields, credentials, invalid resources and unsupported providers.
    pub fn parse(yaml: &str) -> Result<Self, ProfileError> {
        Self::parse_inner(yaml, false)
    }

    /// Read labelled visual fixtures, never use this to authorize provisioning.
    /// # Errors
    /// Rejects malformed fixture profiles.
    pub fn parse_design_fixture(yaml: &str) -> Result<Self, ProfileError> {
        Self::parse_inner(yaml, true)
    }

    fn parse_inner(yaml: &str, fixture: bool) -> Result<Self, ProfileError> {
        let config: Self = serde_yaml::from_str(yaml).map_err(|_| ProfileError::Yaml)?;
        if config.version != 1 || !config.profiles.contains_key(&config.default) || config.profiles.is_empty() {
            return Err(ProfileError::Invalid(
                "cloud.yml requires version 1 and a named default profile",
            ));
        }
        for (name, profile) in &config.profiles {
            if !valid_id(name) {
                return Err(ProfileError::Invalid("Invalid profile name"));
            }
            profile.validate(fixture)?;
        }
        Ok(config)
    }
}
impl Profile {
    /// # Errors
    /// Rejects invalid resources, unsafe build paths and unsupported runtime contracts.
    pub fn validate(&self, design_fixture: bool) -> Result<(), ProfileError> {
        if self.provider != "runpod" && !(design_fixture && matches!(self.provider.as_str(), "daytona" | "fly")) {
            return Err(ProfileError::Invalid(
                "Only RunPod can deploy workers; Daytona and Fly.io are design fixtures",
            ));
        }
        if self.cpu == 0 || self.memory_gb == 0 || self.storage.container_gb == 0 || self.storage.volume_gb == 0 {
            return Err(ProfileError::Invalid("CPU, memory and storage must be positive"));
        }
        if self.bootstrap.contract_version != 1 || !(10..=3600).contains(&self.bootstrap.readiness_seconds) {
            return Err(ProfileError::Invalid(
                "Unsupported worker contract or readiness timeout",
            ));
        }
        if !valid_image(&self.image) {
            return Err(ProfileError::Invalid("Invalid registry image reference"));
        }
        if let Some(build) = &self.build
            && (!repository_path(&build.context)
                || !repository_path(&build.dockerfile)
                || build.platform != "linux/amd64")
        {
            return Err(ProfileError::Invalid(
                "Build paths must stay in the repository; platform must be linux/amd64",
            ));
        }
        Ok(())
    }
}
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
#[must_use]
pub fn valid_image(image: &str) -> bool {
    !image.is_empty()
        && image.len() < 512
        && !image.starts_with('-')
        && !image.contains("://")
        && image
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-:@".contains(&b))
        && (!image.contains('@')
            || image.split_once("@sha256:").is_some_and(|(name, hash)| {
                !name.is_empty() && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
            }))
}
fn repository_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', '\0', '\n'])
        && !Path::new(value).is_absolute()
        && Path::new(value)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}
pub const EXAMPLE: &str = include_str!("../examples/cloud.yml");
pub const DESIGN_EXAMPLE: &str = include_str!("../examples/design-fixtures.yml");

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_build_selection() {
        let config = CloudConfig::parse(EXAMPLE).unwrap();
        assert!(!config.profiles[&config.default].gpu);
        assert!(config.profiles["gpu"].gpu);
        assert!(config.profiles["development"].build.is_some());
        assert!(config.profiles["image-only"].build.is_none());
        assert_eq!(config.profiles["image-only"].storage, Storage::default());
    }
    #[test]
    fn omitted_capabilities_keep_legacy_defaults_in_yaml_and_saved_profiles() {
        let config = CloudConfig::parse(EXAMPLE).unwrap();
        let mut json = serde_json::to_value(&config.profiles["image-only"]).unwrap();
        json.as_object_mut().unwrap().remove("capabilities");
        let profile: Profile = serde_json::from_value(json).unwrap();
        assert_eq!(profile.capabilities, crate::Capabilities::default());
        let yaml = "version: 1\ndefault: min\nprofiles:\n  min:\n    provider: runpod\n    image: example.com/worker\n    cpu: 1\n    memory_gb: 1\n    capabilities: {}\n";
        let minimal = CloudConfig::parse(yaml).unwrap();
        assert!(minimal.profiles["min"].capabilities.agents.is_empty());
        assert!(minimal.profiles["min"].capabilities.browsers.is_empty());
        assert!(!minimal.profiles["min"].capabilities.desktop);
    }
    #[test]
    fn rejects_unsupported_and_secret_bearing_input_without_echoing() {
        for yaml in [
            EXAMPLE.replace("provider: runpod", "provider: daytona"),
            EXAMPLE.replace("cpu: 4", "cpu: 0"),
            EXAMPLE.replace("context: .", "context: ../private"),
            EXAMPLE.replace("default: development", "default: missing"),
            format!("{EXAMPLE}\napi_key: TOP_SECRET"),
        ] {
            let error = CloudConfig::parse(&yaml).unwrap_err().to_string();
            assert!(!error.contains("TOP_SECRET"));
        }
        assert!(CloudConfig::parse(DESIGN_EXAMPLE).is_err());
        assert!(CloudConfig::parse_design_fixture(DESIGN_EXAMPLE).is_ok());
    }
}
