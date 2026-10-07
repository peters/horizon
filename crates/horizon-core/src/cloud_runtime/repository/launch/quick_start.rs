//! A built-in image-only profile for a repository with no `.horizon/cloud.yml`: the
//! public CPU base worker, which needs no registry login and no image build.
use super::{CloudConfig, Error, Path, Runner};

macro_rules! image {
    () => {
        "ghcr.io/peters/horizon-worker-base:proto-5108502"
    };
}

/// The public base worker image. Anonymous pulls work, so the provider needs no
/// registry credential. Deployment still resolves and pins its digest and runs the
/// worker contract check before it allocates compute.
pub const IMAGE: &str = image!();

/// The smallest `RunPod` CPU size: 2 vCPU with 2 GB per vCPU (`cpu3c`) can hold
/// 20 GB of container disk. The capabilities are those the base image is built with
/// (`examples/cloud-worker/build-base-image.sh`).
const PROFILE: &str = concat!(
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

/// The quick-start configuration for `repository` at `revision`. A repository that
/// commits its own `.horizon/cloud.yml` keeps it: quick start never replaces it.
pub(super) fn config(repository: &Path, revision: &str, runner: &Runner<'_>) -> super::super::Result<CloudConfig> {
    if super::committed_config(repository, revision, runner)?.is_some() {
        return Err(Error::Invalid(
            "This commit has its own .horizon/cloud.yml. Quick start is only for a repository without one.",
        ));
    }
    builtin()
}

fn builtin() -> super::super::Result<CloudConfig> {
    CloudConfig::parse(PROFILE).map_err(|_| Error::Invalid("The built-in quick-start profile is invalid"))
}

#[cfg(test)]
mod tests {
    use super::super::{
        Configuration,
        tests::{git, repository},
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

    #[test]
    fn the_builtin_profile_is_an_image_only_runpod_cpu_profile_on_the_public_base() {
        let config = builtin().unwrap();
        let profile = &config.profiles[&config.default];
        assert_eq!(profile.provider, "runpod");
        assert_eq!(profile.image, IMAGE);
        assert!(profile.build.is_none(), "quick start never builds an image");
        assert!(!profile.gpu);
        assert_eq!((profile.cpu, profile.memory_gb), (2, 4));
        assert!(config.source.is_default() && config.companions.is_empty());
    }

    #[test]
    fn a_repository_without_configuration_gets_the_builtin_profile() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        git(temp.path(), &["rm", "--quiet", ".horizon/cloud.yml"]);
        git(
            temp.path(),
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
        let prepared = load(temp.path()).unwrap();
        assert_eq!(prepared.config.default, "quick-start");
        assert_eq!(prepared.config.profiles["quick-start"].image, IMAGE);
    }

    #[test]
    fn a_committed_configuration_is_never_replaced() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let error = load(temp.path()).err().unwrap().to_string();
        assert!(error.contains("its own .horizon/cloud.yml"), "{error}");
    }
}
