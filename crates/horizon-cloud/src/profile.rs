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
    /// Portable declarations only; selecting and authorizing a target is machine-local.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub companions: BTreeMap<String, crate::companions::Declaration>,
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
    /// Minutes without agent activity before a dedicated worker stops itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_stop_minutes: Option<u16>,
    /// Lowest CUDA version, as `major.minor`, a GPU host must offer to run the image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_cuda_version: Option<String>,
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
        crate::companions::validate_declarations(&config.companions)?;
        for (name, profile) in &config.profiles {
            if !valid_id(name) {
                return Err(ProfileError::Invalid("Invalid profile name"));
            }
            profile.validate(fixture)?;
            if profile.provider == "runpod"
                && !profile.gpu
                && !crate::runpod::volumes::REQUEST_SIZE_GB.contains(&u32::from(profile.storage.volume_gb))
            {
                return Err(ProfileError::Invalid(crate::runpod::volumes::INVALID_REQUEST_SIZE));
            }
            if profile.provider == crate::hetzner::PROVIDER
                && !crate::hetzner::volumes::SIZE_GB.contains(&u32::from(profile.storage.volume_gb))
            {
                return Err(ProfileError::Invalid(
                    "A Hetzner workspace volume must be between 10 and 10,240 GB",
                ));
            }
        }
        Ok(config)
    }

    /// Companions on their own clouds, the only ones that take part in selection and SSH grants.
    pub fn cloud_companions(&self) -> impl Iterator<Item = (&str, &crate::companions::Declaration)> {
        self.companions_placed(crate::companions::Placement::Cloud)
    }

    /// Sibling repositories checked out next to this one on the same worker.
    pub fn same_worker_siblings(&self) -> impl Iterator<Item = (&str, &crate::companions::Declaration)> {
        self.companions_placed(crate::companions::Placement::SameWorker)
    }

    fn companions_placed(
        &self,
        placement: crate::companions::Placement,
    ) -> impl Iterator<Item = (&str, &crate::companions::Declaration)> {
        self.companions
            .iter()
            .filter(move |(_, declaration)| declaration.placement == placement)
            .map(|(alias, declaration)| (alias.as_str(), declaration))
    }
}
impl Profile {
    /// # Errors
    /// Rejects invalid resources, unsafe build paths and unsupported runtime contracts.
    pub fn validate(&self, design_fixture: bool) -> Result<(), ProfileError> {
        self.capabilities.validate()?;
        let deployable = matches!(self.provider.as_str(), "runpod" | crate::hetzner::PROVIDER);
        let fixture = design_fixture && matches!(self.provider.as_str(), "daytona" | "fly");
        if !(deployable || fixture) {
            return Err(ProfileError::Invalid(
                "Supported providers are RunPod and Hetzner; Daytona and Fly.io are design fixtures",
            ));
        }
        if self.provider == crate::hetzner::PROVIDER {
            if self.gpu {
                return Err(ProfileError::Invalid(
                    "Hetzner has no GPU workers; use RunPod for a GPU profile",
                ));
            }
            if self.capabilities.browserstack.is_some() {
                return Err(ProfileError::Invalid(
                    "Hosted devices are not available on Hetzner clouds yet",
                ));
            }
        }
        if self.cpu == 0 || self.memory_gb == 0 || self.storage.container_gb == 0 || self.storage.volume_gb == 0 {
            return Err(ProfileError::Invalid("CPU, memory and storage must be positive"));
        }
        if self.bootstrap.contract_version != 1 || !(10..=3600).contains(&self.bootstrap.readiness_seconds) {
            return Err(ProfileError::Invalid(
                "Unsupported worker contract or readiness timeout",
            ));
        }
        if let Some(minutes) = self.idle_stop_minutes {
            if !IDLE_STOP_MINUTES.contains(&minutes) {
                return Err(ProfileError::Invalid("idle_stop_minutes must be between 10 and 1440"));
            }
            // A stopped worker cannot release hosted devices it still holds.
            if self.capabilities.browserstack.is_some() {
                return Err(ProfileError::Invalid(
                    "idle_stop_minutes cannot be combined with hosted devices",
                ));
            }
        }
        if let Some(version) = &self.min_cuda_version {
            if cuda_version(version).is_none() {
                return Err(ProfileError::Invalid(
                    "min_cuda_version must be major.minor, such as 12.8",
                ));
            }
            if !self.gpu {
                return Err(ProfileError::Invalid("min_cuda_version requires gpu: true"));
            }
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
/// Worker environment variable carrying a dedicated worker's idle period in minutes.
pub const IDLE_STOP_ENVIRONMENT_KEY: &str = "HORIZON_IDLE_STOP_MINUTES";
/// Accepted idle periods: long enough to outlast a quiet build, at most one day.
pub const IDLE_STOP_MINUTES: std::ops::RangeInclusive<u16> = 10..=1440;
/// A CUDA version written `major.minor`, as numbers so that 12.11 is above 12.2.
#[must_use]
pub(crate) fn cuda_version(value: &str) -> Option<(u16, u16)> {
    let number = |part: &str| {
        (!part.is_empty() && part.len() <= 4 && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    let (major, minor) = value.split_once('.')?;
    Some((number(major)?, number(minor)?))
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
    fn new_cpu_profiles_enforce_network_volume_limits_without_changing_gpu_storage() {
        for size in [1, 9, 10, 4000, 4001] {
            for gpu in [false, true] {
                let mut config = CloudConfig::parse(EXAMPLE).unwrap();
                let profile = config.profiles.get_mut("image-only").unwrap();
                profile.storage.volume_gb = size;
                profile.gpu = gpu;
                // Saved profiles remain valid for reconciliation and cleanup.
                assert!(profile.validate(false).is_ok());
                let yaml = serde_yaml::to_string(&config).unwrap();
                let accepted = gpu || (10..=4000).contains(&size);
                assert_eq!(CloudConfig::parse(&yaml).is_ok(), accepted, "size={size}, gpu={gpu}");
                assert_eq!(CloudConfig::parse_design_fixture(&yaml).is_ok(), accepted);
            }
        }
        let mut fixture = CloudConfig::parse_design_fixture(DESIGN_EXAMPLE).unwrap();
        for profile in fixture.profiles.values_mut() {
            if profile.provider != "runpod" {
                profile.storage.volume_gb = 1;
            }
        }
        assert!(CloudConfig::parse_design_fixture(&serde_yaml::to_string(&fixture).unwrap()).is_ok());
    }

    #[test]
    fn hetzner_profiles_are_cpu_only_with_hetzner_volume_limits() {
        for (size, accepted) in [(9, false), (10, true), (4001, true), (10_240, true), (10_241, false)] {
            let mut config = CloudConfig::parse(EXAMPLE).unwrap();
            config.profiles.retain(|name, _| name == "image-only");
            config.default = "image-only".into();
            let profile = config.profiles.get_mut("image-only").unwrap();
            profile.provider = crate::hetzner::PROVIDER.into();
            profile.storage.volume_gb = size;
            let yaml = serde_yaml::to_string(&config).unwrap();
            assert_eq!(CloudConfig::parse(&yaml).is_ok(), accepted, "size={size}");
        }
        let mut profile = CloudConfig::parse(EXAMPLE)
            .unwrap()
            .profiles
            .remove("image-only")
            .unwrap();
        profile.provider = crate::hetzner::PROVIDER.into();
        assert!(profile.validate(false).is_ok());
        profile.gpu = true;
        assert!(profile.validate(false).is_err(), "Hetzner has no hourly GPUs");
        profile.gpu = false;
        profile.capabilities.browserstack = Some(crate::BrowserStack {
            provider: crate::BrowserStack::default_provider(),
            targets: ["ios_phone".into()].into(),
            local_ports: [8080].into(),
        });
        assert!(
            profile.validate(false).is_err(),
            "hosted devices are not wired for Hetzner"
        );
    }

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
    fn remote_targets_require_explicit_valid_names_and_ports_without_secrets() {
        let mut profile = CloudConfig::parse(EXAMPLE)
            .unwrap()
            .profiles
            .remove("image-only")
            .unwrap();
        profile.capabilities.browserstack = Some(crate::BrowserStack {
            provider: crate::BrowserStack::default_provider(),
            targets: ["ios_phone".into(), "android_phone".into()].into(),
            local_ports: [8080].into(),
        });
        assert!(profile.validate(false).is_ok());
        profile
            .capabilities
            .browserstack
            .as_mut()
            .unwrap()
            .local_ports
            .insert(0);
        assert!(profile.validate(false).is_err());
        profile.capabilities.browserstack.as_mut().unwrap().local_ports.clear();
        profile.capabilities.browserstack.as_mut().unwrap().provider.clear();
        assert!(profile.validate(false).is_err());
        assert!(
            serde_json::from_str::<crate::Capabilities>(
                r#"{"browserstack":{"targets":["phone"],"access_key":"private-value"}}"#
            )
            .is_err()
        );
    }
    #[test]
    fn idle_stop_is_optional_bounded_and_excludes_hosted_devices() {
        let mut config = CloudConfig::parse(EXAMPLE).unwrap();
        assert!(
            config
                .profiles
                .values()
                .all(|profile| profile.idle_stop_minutes.is_none())
        );
        let yaml = EXAMPLE.replace("    # idle_stop_minutes: 30", "    idle_stop_minutes: 30");
        assert_eq!(
            CloudConfig::parse(&yaml).unwrap().profiles["development"].idle_stop_minutes,
            Some(30)
        );
        let mut profile = config.profiles.remove("image-only").unwrap();
        let saved = serde_json::to_value(&profile).unwrap();
        assert!(saved.get("idle_stop_minutes").is_none());
        assert_eq!(serde_json::from_value::<Profile>(saved).unwrap(), profile);
        for (minutes, accepted) in [(9, false), (10, true), (30, true), (1440, true), (1441, false)] {
            profile.idle_stop_minutes = Some(minutes);
            assert_eq!(profile.validate(false).is_ok(), accepted, "minutes={minutes}");
        }
        profile.idle_stop_minutes = Some(30);
        profile.capabilities.browserstack = Some(crate::BrowserStack {
            provider: crate::BrowserStack::default_provider(),
            targets: ["ios_phone".into()].into(),
            local_ports: [8080].into(),
        });
        assert!(profile.validate(false).is_err());
    }
    #[test]
    fn a_cuda_floor_is_optional_numeric_and_only_for_gpu_profiles() {
        let config = CloudConfig::parse(EXAMPLE).unwrap();
        assert!(
            config
                .profiles
                .values()
                .all(|profile| profile.min_cuda_version.is_none())
        );
        let yaml = EXAMPLE.replace("    # min_cuda_version: \"12.8\"", "    min_cuda_version: \"12.8\"");
        assert_eq!(
            CloudConfig::parse(&yaml).unwrap().profiles["gpu"]
                .min_cuda_version
                .as_deref(),
            Some("12.8")
        );
        let mut profile = config.profiles["gpu"].clone();
        let saved = serde_json::to_value(&profile).unwrap();
        assert!(saved.get("min_cuda_version").is_none());
        assert_eq!(serde_json::from_value::<Profile>(saved).unwrap(), profile);
        for (version, accepted) in [
            ("12.8", true),
            ("13.0", true),
            ("12.11", true),
            ("12", false),
            ("12.", false),
            (".8", false),
            ("12.8.1", false),
            ("v12.8", false),
            ("12.8 ", false),
            ("12,8", false),
            ("12345.0", false),
        ] {
            profile.min_cuda_version = Some(version.into());
            assert_eq!(profile.validate(false).is_ok(), accepted, "version={version}");
        }
        profile.min_cuda_version = Some("12.8".into());
        profile.gpu = false;
        assert_eq!(
            profile.validate(false).unwrap_err().to_string(),
            "min_cuda_version requires gpu: true"
        );
        assert!(cuda_version("12.11") > cuda_version("12.2"));
        assert!(cuda_version("13.0") > cuda_version("12.11"));
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
