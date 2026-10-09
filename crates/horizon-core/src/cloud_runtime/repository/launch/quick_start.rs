//! A built-in image-only profile for a repository with no `.horizon/cloud.yml`: the
//! public CPU base worker, which needs no registry login, no image build and no
//! local Docker.
use super::{CloudConfig, Error, Path, Runner};
use horizon_cloud::{Capabilities, Profile};

// The pin, by digest, so a Horizon build always starts the image it was tested with.
// The Worker images workflow publishes the base image from main and prints the line
// to put here; docs/release-flow.md describes the update, which also updates CONTRACT.
macro_rules! image {
    () => {
        "ghcr.io/peters/horizon-worker-base@sha256:fb0767fd2d98c2f7936c018902a259fc568765faf50d0cecf9ed20bad9adc922"
    };
}

/// The public base worker image. Anonymous pulls work, so the provider needs no
/// registry credential. Its worker contract is [`CONTRACT`], so deployment needs no
/// local Docker to check it before it allocates compute.
pub const IMAGE: &str = image!();

/// What the worker check of [`IMAGE`] reports for the capabilities of the built-in
/// profile with `--git-auth`.
///
/// Horizon uses this report instead of running the check in local Docker, because
/// nothing it could learn locally can differ: a digest names exactly one image, and
/// the Worker images workflow runs this check, the marker check and the host key
/// check on that image, with these capabilities, before it publishes it. The
/// worker still runs its own check when it starts. An ignored test compares this
/// report with the pinned image in local Docker.
pub const CONTRACT: &str = "horizon-git-auth-contract=1
horizon-git-auth-contract=2
horizon-idle-stop-contract=1
horizon-idle-report-contract=1
horizon-siblings-contract=1
horizon-session-env-contract=1
horizon-gpu-lock-contract=1
horizon-source-shallow-contract=1
horizon-source-lfs-selection-contract=1
horizon-tailnet-contract=1
horizon-tailnet-contract=2
horizon-worker-contract=1
horizon-source-contract=1
horizon-capabilities-contract=1
horizon-session-restart-contract=1
horizon-shared-checkout-contract=1
horizon-prepare-checkout-contract=1
";

/// The worker check report Horizon uses for `image` with `capabilities` instead of a
/// local check: [`CONTRACT`] for [`IMAGE`] with the capabilities of the built-in
/// profile, which the publishing workflow checked. Any other image or selection,
/// such as a committed profile that asks the base image for other tools, gets `None`
/// and keeps the local check.
#[must_use]
pub fn trusted_contract(image: &str, capabilities: &Capabilities) -> Option<&'static str> {
    (image == IMAGE
        && builtin().is_ok_and(|config| {
            config
                .profiles
                .get(PROFILE)
                .is_some_and(|profile| profile.capabilities == *capabilities)
        }))
    .then_some(CONTRACT)
}

/// Whether `profile` runs the public base image without a recipe, as a quick-start
/// cloud does. Its rebuild moves it to [`IMAGE`], the image this Horizon version pins.
#[must_use]
pub fn on_public_base(profile: &Profile) -> bool {
    profile.build.is_none() && is_public_base(&profile.image)
}

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
    fn the_trusted_report_is_the_current_checker_contract_and_satisfies_the_builtin_profile() {
        let checker = include_str!("../../../../../../examples/cloud-worker/horizon-worker-check");
        let lines: Vec<_> = CONTRACT.lines().collect();
        assert!(!lines.is_empty());
        for line in &lines {
            assert!(
                line.starts_with("horizon-") && line.contains("-contract=") && checker.contains(line),
                "{line} is a marker that the checker prints"
            );
        }
        let unique: std::collections::BTreeSet<_> = lines.iter().collect();
        assert_eq!(unique.len(), lines.len(), "each marker once");
        let profile = &builtin().unwrap().profiles[PROFILE];
        let report = trusted_contract(IMAGE, &profile.capabilities).unwrap();
        crate::cloud_runtime::worker_contract::validate(report, &profile.capabilities, true, true).unwrap();
        crate::cloud_runtime::worker_contract::validate_idle_report(report, true).unwrap();
        let contract = crate::cloud_runtime::WorkerContract::reported(report);
        assert!(contract.tailnet && contract.session_restart && contract.prepare_checkout);
        assert!(contract.pinned_submodules && contract.lfs_selection);
    }

    #[test]
    fn only_the_pin_with_the_checked_capabilities_is_trusted() {
        let capabilities = builtin().unwrap().profiles[PROFILE].capabilities.clone();
        assert!(trusted_contract(IMAGE, &capabilities).is_some());
        let other_digest = format!("{REPOSITORY}@sha256:{}", "0".repeat(64));
        for image in [
            other_digest.as_str(),
            "ghcr.io/peters/horizon-worker-base:cpu",
            "registry.example/worker",
        ] {
            assert!(trusted_contract(image, &capabilities).is_none(), "{image}");
        }
        let mut fewer = capabilities.clone();
        fewer.desktop = false;
        let mut more = capabilities;
        more.agents.insert(horizon_cloud::Agent::Grok);
        for selection in [fewer, more, Capabilities::default()] {
            assert!(trusted_contract(IMAGE, &selection).is_none(), "{selection:?}");
        }
    }

    #[test]
    fn a_cloud_on_the_base_image_without_a_recipe_rebuilds_on_the_pin() {
        let mut profile = builtin().unwrap().profiles[PROFILE].clone();
        assert!(on_public_base(&profile));
        profile.image = format!("{REPOSITORY}@sha256:{}", "0".repeat(64));
        assert!(on_public_base(&profile), "an earlier pin");
        profile.image = "registry.example/worker@sha256:".to_owned() + &"0".repeat(64);
        assert!(!on_public_base(&profile));
        profile.image = IMAGE.into();
        profile.build =
            Some(serde_json::from_value(serde_json::json!({"context": ".", "dockerfile": "Dockerfile"})).unwrap());
        assert!(!on_public_base(&profile), "a recipe rebuilds from the repository");
    }

    #[test]
    #[ignore = "pulls the pinned image and runs its worker check in local Docker; run it when the pin changes"]
    fn the_trusted_report_is_what_the_pinned_image_reports() {
        let capabilities = &builtin().unwrap().profiles[PROFILE].capabilities;
        let output = std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "--pull",
                "missing",
                "--platform",
                "linux/amd64",
                "--network=none",
            ])
            .args(["--entrypoint", "/usr/local/bin/horizon-worker-check", "--env"])
            .arg(crate::cloud_runtime::worker_contract::environment(capabilities).unwrap())
            .args([IMAGE, "--git-auth"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let report = std::str::from_utf8(&output.stdout).unwrap();
        // With --nocapture, the report to put in CONTRACT when the pin changes.
        println!("{report}");
        let reported: std::collections::BTreeSet<_> = report.lines().map(str::to_owned).collect();
        let trusted: std::collections::BTreeSet<_> = CONTRACT.lines().map(str::to_owned).collect();
        assert_eq!(reported, trusted);
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
