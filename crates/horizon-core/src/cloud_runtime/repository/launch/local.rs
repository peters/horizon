//! Local launch settings never change the committed source or build context.
use super::{CloudConfig, Error, Path, Runner};
use std::{fs::File, io::Read};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

pub(super) fn read(repository: &Path, runner: &Runner<'_>) -> super::super::Result<CloudConfig> {
    runner.cancel.check()?;
    let path = repository.join(".horizon/cloud.yml");
    if std::fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
        return Err(Error::Invalid("Local .horizon/cloud.yml must be a regular file"));
    }
    let file = File::open(path).map_err(|_| {
        Error::Invalid("No readable local .horizon/cloud.yml. Prepare local image-only settings first.")
    })?;
    if !file.metadata()?.is_file() {
        return Err(Error::Invalid("Local .horizon/cloud.yml must be a regular file"));
    }
    let mut yaml = String::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut yaml)?;
    runner.cancel.check()?;
    if yaml.len() as u64 > MAX_CONFIG_BYTES {
        return Err(Error::Invalid("Local .horizon/cloud.yml exceeds the 1 MiB limit"));
    }
    let mut config = CloudConfig::parse(&yaml).map_err(|_| Error::Invalid(super::INVALID_CONFIG))?;
    if !config.source.is_default() || !config.companions.is_empty() {
        return Err(Error::Invalid(
            "Local image-only settings cannot change source packaging or declare companion repositories",
        ));
    }
    if config
        .profiles
        .get(&config.default)
        .is_some_and(|profile| profile.build.is_some())
    {
        return Err(Error::Invalid(
            "The local default profile needs an existing registry image and no build section. Choose an image-only default in .horizon/cloud.yml.",
        ));
    }
    config.profiles.retain(|_, profile| profile.build.is_none());
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::super::{
        Configuration,
        tests::{CONFIG, git, read as committed, repository},
    };
    use super::*;

    fn load(path: &Path) -> super::super::super::Result<super::super::Prepared> {
        let cancel = super::super::super::super::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        super::super::prepare_with_configuration(path.to_str().unwrap(), "HEAD", Configuration::LocalImageOnly, &runner)
    }

    #[test]
    fn untracked_settings_load_without_changing_the_committed_source() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let original = committed(temp.path()).unwrap();
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
        std::fs::create_dir_all(temp.path().join(".horizon")).unwrap();
        std::fs::write(
            temp.path().join(".horizon/cloud.yml"),
            CONFIG.replace("cpu: 4", "cpu: 8"),
        )
        .unwrap();
        let prepared = load(temp.path()).unwrap();
        assert_ne!(prepared.revision, original.revision);
        assert_eq!(prepared.config.profiles["dev"].cpu, 8);
        assert!(committed(temp.path()).is_err());
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(temp.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(String::from_utf8(status.stdout).unwrap().contains("?? .horizon/"));
    }

    #[test]
    fn reload_reads_edits_while_committed_mode_keeps_its_revision() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let original = committed(temp.path()).unwrap();
        for cpu in [8, 16] {
            std::fs::write(
                temp.path().join(".horizon/cloud.yml"),
                CONFIG.replace("cpu: 4", &format!("cpu: {cpu}")),
            )
            .unwrap();
            let prepared = load(temp.path()).unwrap();
            assert_eq!(prepared.revision, original.revision);
            assert_eq!(prepared.config.profiles["dev"].cpu, cpu);
            assert_eq!(committed(temp.path()).unwrap().config.profiles["dev"].cpu, 4);
        }
    }

    #[test]
    fn build_profiles_are_not_offered_as_local_images() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let build = format!("{CONFIG}    build:\n      context: .\n      dockerfile: Dockerfile\n");
        std::fs::write(temp.path().join(".horizon/cloud.yml"), &build).unwrap();
        assert!(
            load(temp.path())
                .err()
                .unwrap()
                .to_string()
                .contains("no build section")
        );
        let mixed = format!(
            "{build}  prebuilt:\n    provider: runpod\n    image: example.invalid/test-worker\n    cpu: 4\n    memory_gb: 8\n    gpu: true\n    capabilities:\n      agents: []\n      browsers: []\n      desktop: true\n"
        );
        std::fs::write(
            temp.path().join(".horizon/cloud.yml"),
            mixed.replace("default: dev", "default: prebuilt"),
        )
        .unwrap();
        let prepared = load(temp.path()).unwrap();
        assert_eq!(prepared.config.default, "prebuilt");
        assert_eq!(prepared.config.profiles.len(), 1);
        let profile = &prepared.config.profiles["prebuilt"];
        assert!(profile.gpu);
        assert!(profile.capabilities.desktop);
        assert!(profile.capabilities.agents.is_empty());
        assert!(profile.capabilities.browsers.is_empty());
    }

    #[test]
    fn malformed_secret_or_source_settings_fail_without_logging_the_input() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        for yaml in [
            "invalid-private-marker".to_owned(),
            format!("{CONFIG}    api_key: private-marker\n"),
            format!("{CONFIG}source:\n  submodule_history: pinned\n"),
            format!("{CONFIG}companions:\n  extra:\n    repository: example/service\n    profile: dev\n"),
        ] {
            std::fs::write(temp.path().join(".horizon/cloud.yml"), yaml).unwrap();
            let error = load(temp.path()).err().unwrap().to_string();
            assert!(!error.contains("private-marker"));
        }
    }

    #[test]
    fn oversized_settings_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        std::fs::write(
            temp.path().join(".horizon/cloud.yml"),
            "#".repeat(usize::try_from(MAX_CONFIG_BYTES).unwrap() + 1),
        )
        .unwrap();
        assert!(load(temp.path()).err().unwrap().to_string().contains("1 MiB"));
    }

    #[test]
    fn a_gpu_build_default_never_falls_back_to_a_cpu_image() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let yaml = format!(
            "{CONFIG}    gpu: true\n    build:\n      context: .\n      dockerfile: Dockerfile\n  cpu-image:\n    provider: runpod\n    image: example.invalid/cpu\n    cpu: 4\n    memory_gb: 8\n"
        );
        std::fs::write(temp.path().join(".horizon/cloud.yml"), yaml).unwrap();
        assert!(
            load(temp.path())
                .err()
                .unwrap()
                .to_string()
                .contains("image-only default")
        );
    }
}
