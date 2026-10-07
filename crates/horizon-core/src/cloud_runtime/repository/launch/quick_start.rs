//! A built-in image-only profile for a repository with no `.horizon/cloud.yml`: the
//! public CPU base worker, which needs no registry login and no image build.
use super::{CloudConfig, Error, Path, Runner};

// The pin, by digest, so a Horizon build always starts the image it was tested with.
// The Worker images workflow publishes the base image from main and prints the line
// to put here; docs/release-flow.md describes the update.
macro_rules! image {
    () => {
        "ghcr.io/peters/horizon-worker-base@sha256:94d85a34632bec1b2819aaf982791b046fbaae70151f7f670e8816be05ed3c18"
    };
}

/// The public base worker image. Anonymous pulls work, so the provider needs no
/// registry credential. Deployment still checks the worker contract before it
/// allocates compute.
pub const IMAGE: &str = image!();

/// The repository of the public base image. Anyone can pull it, so Horizon never
/// attaches a registry login to it, whatever logins this machine has saved.
pub const REPOSITORY: &str = "ghcr.io/peters/horizon-worker-base";

/// Whether `image` is the public base image, at any tag or digest.
#[must_use]
pub fn is_public_base(image: &str) -> bool {
    image
        .strip_prefix(REPOSITORY)
        .is_some_and(|rest| rest.starts_with(['@', ':']))
}

/// The name of the built-in profile.
pub const PROFILE: &str = "quick-start";

/// The smallest `RunPod` CPU size: 2 vCPU with 2 GB per vCPU (`cpu3c`) can hold
/// 20 GB of container disk. The capabilities are those the base image is built with
/// (`examples/cloud-worker/build-base-image.sh`).
const CONFIG: &str = concat!(
    "version: 1
default: quick-start
profiles:
  quick-start:
    provider: runpod
    image: ",
    image!(),
    "
    min_cpu: 2
    min_memory_gb: 4
    gpu: false
    storage:
      container_gb: 20
      volume_gb: 20
    bootstrap:
      readiness_seconds: 900
    capabilities:
      agents: [claude, codex]
      browsers: [chromium]
      desktop: true
"
);

/// The quick-start configuration for `repository` at `revision`. A commit with its own
/// `.horizon/cloud.yml` keeps it: quick start never replaces committed settings.
pub(super) fn config(repository: &Path, revision: &str, runner: &Runner<'_>) -> super::super::Result<CloudConfig> {
    if super::committed_config(repository, revision, runner)?.is_some() {
        return Err(Error::Invalid(COMMITTED));
    }
    builtin()
}

/// The refusal of quick start for a commit that has its own configuration.
const COMMITTED: &str = "This commit has its own .horizon/cloud.yml. Quick start is only for a repository without one.";

/// Whether `error` refuses quick start because the commit has its own configuration,
/// which then applies instead.
#[must_use]
pub fn is_refused(error: &Error) -> bool {
    matches!(error, Error::Invalid(message) if *message == COMMITTED)
}

/// The built-in configuration with its one profile, [`PROFILE`].
/// # Errors
/// Only if the built-in text stopped parsing, which the tests rule out.
pub fn builtin() -> super::super::Result<CloudConfig> {
    CloudConfig::parse(CONFIG).map_err(|_| Error::Invalid("The built-in quick-start profile is invalid"))
}

#[cfg(test)]
mod tests {
    use super::super::{
        Configuration,
        tests::{CONFIG as COMMITTED_CONFIG, git, repository},
    };
    use super::*;

    fn load(path: &Path) -> super::super::super::Result<super::super::Prepared> {
        let cancel = super::super::super::super::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        super::super::prepare_with_configuration(path.to_str().unwrap(), "HEAD", Configuration::QuickStart, &runner)
    }

    fn remove_configuration(path: &Path) {
        git(path, &["rm", "--quiet", ".horizon/cloud.yml"]);
        git(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "Remove configuration",
            ],
        );
    }

    #[test]
    fn the_image_is_the_public_base_pinned_by_digest() {
        let (repository, digest) = IMAGE.split_once("@sha256:").unwrap();
        assert_eq!(repository, REPOSITORY);
        assert_eq!(digest.len(), 64);
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(is_public_base(IMAGE) && is_public_base("ghcr.io/peters/horizon-worker-base:cpu"));
        for other in [
            "ghcr.io/peters/horizon-worker-base-evil:cpu",
            "ghcr.io/peters/horizon-worker-helpers:main",
            "registry.example/ghcr.io/peters/horizon-worker-base:cpu",
            REPOSITORY,
        ] {
            assert!(!is_public_base(other), "{other}");
        }
    }

    #[test]
    fn the_builtin_profile_is_an_image_only_runpod_cpu_profile_on_the_public_base() {
        let config = builtin().unwrap();
        assert_eq!(config.default, PROFILE);
        let profile = &config.profiles[PROFILE];
        assert_eq!(profile.provider, "runpod");
        assert_eq!(profile.image, IMAGE);
        assert!(profile.build.is_none(), "quick start never builds an image");
        assert!(!profile.gpu);
        assert_eq!((profile.cpu, profile.memory_gb), (2, 4));
        assert!(config.source.is_default() && config.companions.is_empty());
        assert!(super::super::creatable(config).is_ok());
    }

    #[test]
    fn the_base_image_recipe_builds_the_capabilities_the_profile_requests() {
        let script = include_str!("../../../../../../examples/cloud-worker/build-base-image.sh");
        let value = |name: &str| {
            let mut values: Vec<_> = script
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name}=")))
                .unwrap()
                .split(',')
                .map(str::to_owned)
                .collect();
            values.sort();
            values
        };
        let capabilities = &builtin().unwrap().profiles[PROFILE].capabilities;
        let mut agents: Vec<_> = capabilities
            .agents
            .iter()
            .map(|agent| agent.as_str().to_owned())
            .collect();
        let mut browsers: Vec<_> = capabilities
            .browsers
            .iter()
            .map(|browser| browser.as_str().to_owned())
            .collect();
        agents.sort();
        browsers.sort();
        assert_eq!(value("agents"), agents);
        assert_eq!(value("browsers"), browsers);
        assert_eq!(value("desktop"), [capabilities.desktop.to_string()]);
    }

    #[test]
    fn a_repository_without_configuration_gets_the_builtin_profile() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        remove_configuration(temp.path());
        let prepared = load(temp.path()).unwrap();
        assert_eq!(prepared.config.default, PROFILE);
        assert_eq!(prepared.config.profiles[PROFILE].image, IMAGE);
    }

    #[test]
    fn a_committed_configuration_is_never_replaced() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let error = load(temp.path()).err().unwrap();
        assert!(is_refused(&error), "{error}");
        // An invalid committed file is still the repository's own settings.
        std::fs::write(temp.path().join(".horizon/cloud.yml"), "private-invalid-marker").unwrap();
        git(temp.path(), &["add", "."]);
        git(
            temp.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "Break configuration",
            ],
        );
        let error = load(temp.path()).err().unwrap().to_string();
        assert!(error.contains("Invalid .horizon/cloud.yml"), "{error}");
    }

    #[test]
    fn an_uncommitted_configuration_does_not_block_quick_start() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        remove_configuration(temp.path());
        std::fs::create_dir_all(temp.path().join(".horizon")).unwrap();
        std::fs::write(temp.path().join(".horizon/cloud.yml"), COMMITTED_CONFIG).unwrap();
        assert_eq!(load(temp.path()).unwrap().config.default, PROFILE);
    }
}
