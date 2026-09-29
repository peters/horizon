//! Portable repository launch intent. Secrets and account bindings are not accepted.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

mod packages;
pub use packages::Packages;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CloudConfig {
    pub version: u32,
    pub default: String,
    pub profiles: BTreeMap<String, Profile>,
    /// Portable declarations only; selecting and authorizing a target is machine-local.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub companions: BTreeMap<String, crate::companions::Declaration>,
    /// How the committed source is packaged for a worker.
    #[serde(default, skip_serializing_if = "Source::is_default")]
    pub source: Source,
}

/// The optional `source` block of `.horizon/cloud.yml`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default)]
    pub submodule_history: SubmoduleHistory,
    #[serde(default, skip_serializing_if = "Lfs::is_empty")]
    pub lfs: Lfs,
    /// Private dependency packages restored on the owner's computer and sent with the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<Packages>,
}

impl Source {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    fn validate(&self) -> Result<(), ProfileError> {
        let patterns = self.lfs.include.iter().chain(&self.lfs.exclude);
        if patterns.clone().count() > Lfs::MAX_PATTERNS
            || patterns.clone().map(|pattern| pattern.chars().count()).sum::<usize>() > Lfs::MAX_TOTAL_CHARS
            || patterns.clone().any(|pattern| !Lfs::valid_pattern(pattern))
        {
            return Err(ProfileError::Invalid(
                "source.lfs allows at most 64 patterns and 8,192 characters in all, each non-empty, at most 256 characters, without commas or Unicode control, format, surrogate or private-use characters",
            ));
        }
        self.packages.as_ref().map_or(Ok(()), Packages::validate)
    }
}

/// The repository's own LFS paths a worker receives, as git-lfs fetch patterns
/// (`lfs.fetchinclude`/`lfs.fetchexclude`). Paths left out stay pointer files on the
/// worker. Submodule LFS content is always sent.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lfs {
    /// Only matching paths; empty means every path.
    #[serde(default)]
    pub include: Vec<String>,
    /// Matching paths are left out, after `include`.
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Lfs {
    const MAX_PATTERNS: usize = 64;
    const MAX_PATTERN_CHARS: usize = 256;
    /// Horizon passes the joined patterns to `git lfs ls-files`; at two UTF-16 units per
    /// character this keeps the call well within Windows' 32,767-unit command line.
    const MAX_TOTAL_CHARS: usize = 8192;

    /// As the worker's check: git-lfs joins patterns with commas, and no Unicode
    /// control, format, surrogate or private-use character belongs in a path pattern.
    /// Unassigned code points depend on the Unicode version, so neither end rejects them.
    fn valid_pattern(pattern: &str) -> bool {
        use unicode_general_category::{GeneralCategory, get_general_category};
        !pattern.is_empty()
            && pattern.chars().count() <= Self::MAX_PATTERN_CHARS
            && !pattern.chars().any(|c| {
                c == ','
                    || matches!(
                        get_general_category(c),
                        GeneralCategory::Control
                            | GeneralCategory::Format
                            | GeneralCategory::Surrogate
                            | GeneralCategory::PrivateUse
                    )
            })
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }
}

/// How much of each pinned submodule's history a worker receives.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubmoduleHistory {
    /// Every commit reachable from the pinned one, so `git log`, `describe` and
    /// `blame` inside a submodule behave as they do locally.
    #[default]
    Full,
    /// Only the pinned commit and its tree. The worker records the submodule as
    /// shallow; history-reading build steps inside it see a single commit.
    Pinned,
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
    /// Minutes without agent activity before a dedicated worker is stopped: by the
    /// worker itself where the provider gives it a credential for that (`RunPod`), by
    /// Horizon while it runs where not (Hetzner); see `provider::IdleStop`.
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
    #[serde(skip_serializing_if = "is_default")]
    pub volume_tier: crate::runpod::volumes::Tier,
}
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}
impl Storage {
    #[must_use]
    pub fn standard_tier(&self) -> bool {
        self.volume_tier == crate::runpod::volumes::Tier::Standard
    }
}
impl Default for Storage {
    fn default() -> Self {
        Self {
            container_gb: 20,
            volume_gb: 20,
            volume_tier: crate::runpod::volumes::Tier::default(),
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
        config.source.validate()?;
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
        if !self.storage.standard_tier() && (self.provider != "runpod" || self.gpu) {
            return Err(ProfileError::Invalid(
                "High-performance network storage requires a RunPod CPU cloud",
            ));
        }
        let supported = matches!(self.provider.as_str(), "runpod" | crate::hetzner::PROVIDER);
        let fixture = design_fixture && matches!(self.provider.as_str(), "daytona" | "fly");
        if !(supported || fixture) {
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
    fn source_block_defaults_to_everything_and_rejects_invalid_selections() {
        let parse = |yaml: &str| serde_yaml::from_str::<Source>(yaml);
        assert_eq!(parse("{}").unwrap().submodule_history, SubmoduleHistory::Full);
        assert_eq!(
            parse("submodule_history: pinned").unwrap().submodule_history,
            SubmoduleHistory::Pinned
        );
        assert!(parse("submodule_history: shallow").is_err());
        assert!(parse("lfs: {paths: []}").is_err());
        let lfs = parse("lfs: {include: [src/**], exclude: ['fixtures/**']}").unwrap().lfs;
        assert_eq!(
            (lfs.include, lfs.exclude),
            (vec!["src/**".into()], vec!["fixtures/**".into()])
        );
        let config =
            |lfs: &str| CloudConfig::parse(&format!("{}\nsource:\n  lfs: {lfs}\n", EXAMPLE.replace("\r\n", "\n")));
        assert!(config("{exclude: ['*.mp4']}").is_ok());
        assert!(
            config(&format!("{{exclude: ['{}']}}", "é".repeat(256))).is_ok(),
            "characters, not bytes"
        );
        assert!(
            config("{exclude: [\"a\\U000e0080b\"]}").is_ok(),
            "unassigned is version-dependent"
        );
        for invalid in [
            "{exclude: ['a,b']}",
            "{exclude: ['']}",
            "{include: [\"a\\nb\"]}",
            "{exclude: [\"a\\x7fb\"]}",
            "{exclude: [\"a\\x85b\"]}",
            "{exclude: [\"a\\u200bb\"]}",
            "{exclude: [\"a\\ue000b\"]}",
        ] {
            assert!(matches!(config(invalid), Err(ProfileError::Invalid(_))), "{invalid}");
        }
        assert!(
            matches!(config("{exclude: 'fixtures/**'}"), Err(ProfileError::Yaml)),
            "a list, not a string"
        );
        assert!(config(&format!("{{exclude: ['{}']}}", "a".repeat(257))).is_err());
        let many = (0..=Lfs::MAX_PATTERNS)
            .map(|index| format!("p{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        assert!(config(&format!("{{exclude: [{many}]}}")).is_err());
        let long = |count: usize| {
            let pattern = format!("'{}'", "\u{10000}".repeat(Lfs::MAX_PATTERN_CHARS));
            config(&format!("{{exclude: [{}]}}", vec![pattern; count].join(", ")))
        };
        assert!(long(Lfs::MAX_TOTAL_CHARS / Lfs::MAX_PATTERN_CHARS).is_ok());
        assert!(
            matches!(
                long(Lfs::MAX_TOTAL_CHARS / Lfs::MAX_PATTERN_CHARS + 1),
                Err(ProfileError::Invalid(_))
            ),
            "the joined patterns fit a Windows command line"
        );
        // A Windows checkout gives the included example CRLF line endings.
        let documented = EXAMPLE.replace("\r\n", "\n");
        let (head, tail) = documented.split_once("# source:\n").unwrap();
        let (block, rest) = tail.split_once("\n\n").unwrap();
        let example = format!("{head}source:\n{}\n\n{rest}", block.replace("#   ", "  "));
        let config = CloudConfig::parse(&example).unwrap();
        assert_eq!(config.source.submodule_history, SubmoduleHistory::Pinned);
        assert_eq!(config.source.lfs.exclude, ["fixtures/video/**"]);
        let packages = config.source.packages.unwrap();
        assert_eq!(
            (packages.restore.join(" "), packages.env.as_str()),
            ("dotnet restore --packages {dir}".to_owned(), "NUGET_PACKAGES")
        );
        assert!(CloudConfig::parse(&documented).unwrap().source.is_default());
        let restore = |line: &str| {
            CloudConfig::parse(&format!(
                "{}\nsource:\n  packages: {line}\n",
                EXAMPLE.replace("\r\n", "\n")
            ))
        };
        assert!(restore("{restore: [tool, '{dir}'], env: CACHE}").is_ok());
        assert!(
            matches!(restore("{restore: [tool], env: CACHE}"), Err(ProfileError::Invalid(_))),
            "the command must name the folder"
        );
        assert!(matches!(
            restore("{restore: [tool, '{dir}'], env: PATH}"),
            Err(ProfileError::Invalid(_))
        ));
        assert!(matches!(
            restore("{restore: [tool, '{dir}'], env: CACHE, shell: true}"),
            Err(ProfileError::Yaml)
        ));
    }

    #[test]
    fn storage_tier_preserves_legacy_encoding_and_requires_cpu_network_storage() {
        let legacy = r#"{"container_gb":20,"volume_gb":20}"#;
        let storage: Storage = serde_json::from_str(legacy).unwrap();
        assert_eq!(storage, Storage::default());
        assert_eq!(serde_json::to_string(&storage).unwrap(), legacy);
        let mut profile = CloudConfig::parse(EXAMPLE)
            .unwrap()
            .profiles
            .remove("image-only")
            .unwrap();
        let legacy = serde_json::to_vec(&profile).unwrap();
        profile.storage.volume_tier = crate::runpod::volumes::Tier::Standard;
        assert_eq!(serde_json::to_vec(&profile).unwrap(), legacy);
        profile.storage.volume_tier = crate::runpod::volumes::Tier::HighPerformance;
        let encoded = serde_json::to_string(&profile).unwrap();
        assert!(encoded.contains("HIGH_PERFORMANCE"));
        assert_eq!(serde_json::from_str::<Profile>(&encoded).unwrap(), profile);
        assert!(profile.validate(false).is_ok());
        profile.gpu = true;
        assert!(profile.validate(false).is_err());
        profile.gpu = false;
        profile.provider = "hetzner".into();
        assert!(profile.validate(false).is_err());
        assert!(profile.validate(true).is_err());
    }

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
