//! Read-only preparation shared by cloud launch callers.
use super::{Error, PathBuf, Runner, resolve_with_runner};
use horizon_cloud::CloudConfig;
use std::{path::Path, process::Command, time::Duration};

mod local;
pub mod quick_start;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Configuration {
    #[default]
    Committed,
    LocalImageOnly,
    /// The built-in profile on the public base image, for a repository without
    /// `.horizon/cloud.yml`.
    QuickStart,
}

pub struct Prepared {
    pub repository: PathBuf,
    pub revision: String,
    pub config: CloudConfig,
}

/// # Errors
/// Distinguishes a missing checkout/configuration from an invalid profile or revision.
pub fn prepare(directory: &str, revision: &str, runner: &Runner<'_>) -> super::Result<Prepared> {
    prepare_with_configuration(directory, revision, Configuration::Committed, runner)
}

/// # Errors
/// Local settings offer existing worker images only; source stays pinned to `revision`.
pub fn prepare_with_configuration(
    directory: &str,
    revision: &str,
    configuration: Configuration,
    runner: &Runner<'_>,
) -> super::Result<Prepared> {
    if directory.trim().is_empty() {
        return Err(Error::Invalid("Choose the workspace repository in Advanced"));
    }
    let path = crate::Config::expand_tilde(directory).canonicalize()?;
    let root = runner.run(
        "Find workspace Git repository",
        Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["rev-parse", "--show-toplevel"]),
        Duration::from_secs(30),
    )?;
    let repository = Path::new(root.trim_end_matches(['\r', '\n'])).canonicalize()?;
    let revision = resolve_with_runner(&repository, if revision.is_empty() { "HEAD" } else { revision }, runner)?;
    let config = match configuration {
        Configuration::Committed => committed_config(&repository, &revision, runner)?.ok_or(Error::Invalid(
            "The selected commit has no readable .horizon/cloud.yml. Commit the cloud configuration, choose another revision, use local image-only settings in More options, or quick start on the public base image.",
        ))?,
        Configuration::LocalImageOnly => local::read(&repository, runner)?,
        Configuration::QuickStart => quick_start::config(&repository, &revision, runner)?,
    };
    let config = creatable(config)?;
    Ok(Prepared {
        repository,
        revision,
        config,
    })
}

/// The profiles the New cloud dialog can create: those a provider Horizon creates
/// clouds on accepts, such as a Hetzner CPU profile, by that provider's description.
fn creatable(mut config: CloudConfig) -> super::Result<CloudConfig> {
    config.profiles.retain(|_, profile| {
        horizon_cloud::provider::by_id(&profile.provider)
            .is_some_and(|provider| provider.creatable && provider.supports(profile))
    });
    if !config.profiles.contains_key(&config.default) {
        config.default = config.profiles.keys().next().cloned().ok_or(Error::Invalid(
            "No profile in this repository's .horizon/cloud.yml can run on a provider Horizon creates clouds on.",
        ))?;
    }
    Ok(config)
}

/// The refusal of a committed `.horizon/cloud.yml` that does not parse.
pub(crate) const INVALID_CONFIG: &str =
    "Invalid .horizon/cloud.yml. Check its syntax, default profile and supported fields.";

/// The `.horizon/cloud.yml` committed at `revision`, or `None` when Git finds no such file
/// at that commit.
/// # Errors
/// Reports cancellation, Git that could not run or timed out, and an invalid configuration.
pub fn committed_config(repository: &Path, revision: &str, runner: &Runner<'_>) -> super::Result<Option<CloudConfig>> {
    // Configuration may contain invalid secret-bearing fields; never stream its blob to progress logs.
    let yaml = Runner {
        cancel: runner.cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
    .run(
        "Read committed cloud configuration",
        Command::new("git").arg("-C").arg(repository).args([
            "cat-file",
            "blob",
            &format!("{revision}:.horizon/cloud.yml"),
        ]),
        Duration::from_secs(30),
    );
    runner.cancel.check()?;
    let yaml = match yaml {
        Ok(yaml) => yaml,
        // Only a failed lookup means the file is missing; anything else is operational.
        Err(Error::Command(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    CloudConfig::parse(&yaml)
        .map(Some)
        .map_err(|_| Error::Invalid(INVALID_CONFIG))
}

/// Read each machine binding once for a configuration, independent of its profile count.
#[must_use]
pub fn ready_profiles(root: &Path, config: &CloudConfig, cancel: &super::super::Cancellation) -> Vec<String> {
    use super::super::settings::{Settings, validate_private_key_file};
    let Ok(settings) = Settings::load(&root.join("settings.json")) else {
        return Vec::new();
    };
    // Each provider whose credential this machine holds; a profile is ready when one
    // of them can run it, as the New cloud dialog then offers that provider.
    let providers: Vec<_> = [
        (settings.credential().is_ok(), &super::super::provider::RUNPOD),
        (
            settings
                .hetzner
                .as_ref()
                .is_some_and(|hetzner| hetzner.credential().is_ok()),
            &super::super::provider::HETZNER,
        ),
    ]
    .into_iter()
    .filter_map(|(bound, provider)| bound.then_some(provider))
    .collect();
    if cancel.check().is_err() || providers.is_empty() || !ssh_ready_with_cancel(&settings.ssh_identity_file, cancel) {
        return Vec::new();
    }
    let agents = [
        ("codex", settings.openai_api_key_file),
        ("claude", settings.anthropic_api_key_file),
    ]
    .map(|(agent, path)| {
        let needed = config
            .profiles
            .values()
            .any(|profile| profile.capabilities.permits_agent(agent));
        (
            agent,
            !needed || path.as_ref().is_none_or(|path| validate_private_key_file(path).is_ok()),
        )
    });
    config
        .profiles
        .iter()
        .filter(|(_, profile)| {
            cancel.check().is_ok()
                && providers.iter().any(|provider| provider.supports(profile))
                && agents
                    .iter()
                    .all(|(agent, ready)| !profile.capabilities.permits_agent(agent) || *ready)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Check the existing worker identity without creating or replacing it.
#[must_use]
pub fn ssh_ready(path: &Path) -> bool {
    ssh_ready_with_cancel(path, &super::super::Cancellation::default())
}

fn ssh_ready_with_cancel(path: &Path, cancel: &super::super::Cancellation) -> bool {
    if super::super::settings::validate_ssh_identity(path).is_err() {
        return false;
    }
    let mut public_path = path.as_os_str().to_os_string();
    public_path.push(".pub");
    let public_path = PathBuf::from(public_path);
    let Ok(meta) = std::fs::metadata(&public_path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > 16 * 1024 {
        return false;
    }
    let Ok(public) = std::fs::read_to_string(public_path) else {
        return false;
    };
    if !horizon_cloud::valid_public_key(public.trim()) {
        return false;
    }
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    let Ok(derived) = runner.run(
        "Validate SSH identity",
        Command::new("ssh-keygen").args(["-y", "-P", "", "-f"]).arg(path),
        Duration::from_secs(5),
    ) else {
        return false;
    };
    let public: Vec<_> = public.split_whitespace().take(2).collect();
    public.len() == 2 && public == derived.split_whitespace().take(2).collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) const CONFIG: &str = "version: 1\ndefault: dev\nprofiles:\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n";

    pub(super) fn git(path: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    pub(super) fn repository(path: &Path) {
        git(path, &["init", "--quiet"]);
        std::fs::create_dir_all(path.join(".horizon")).unwrap();
        std::fs::write(path.join(".horizon/cloud.yml"), CONFIG).unwrap();
        git(path, &["add", "."]);
        git(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "Add fixture",
            ],
        );
    }
    pub(super) fn read(path: &Path) -> super::super::Result<Prepared> {
        read_revision(path, "HEAD")
    }
    fn read_revision(path: &Path, revision: &str) -> super::super::Result<Prepared> {
        prepare(
            path.to_str().unwrap(),
            revision,
            &Runner {
                cancel: &super::super::super::Cancellation::default(),
                emit: &|_| {},
                secrets: Vec::new(),
            },
        )
    }
    #[test]
    fn the_new_cloud_dialog_offers_hetzner_and_runpod_profiles() {
        let yaml = "version: 1\ndefault: cheap\nprofiles:\n  cheap:\n    provider: hetzner\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n  dev:\n    provider: runpod\n    image: example.invalid/worker\n    cpu: 4\n    memory_gb: 8\n";
        let config = creatable(CloudConfig::parse(yaml).unwrap()).unwrap();
        assert_eq!(config.profiles.keys().collect::<Vec<_>>(), ["cheap", "dev"]);
        assert_eq!(
            config.default, "cheap",
            "a Hetzner CPU profile is usable as the default"
        );
    }

    #[test]
    fn ssh_readiness_requires_a_matching_public_companion() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("worker.identity");
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        assert!(ssh_ready(&key));
        let public = temp.path().join("worker.identity.pub");
        let saved = std::fs::read(&public).unwrap();
        std::fs::remove_file(&public).unwrap();
        assert!(!ssh_ready(&key));
        std::fs::write(&public, "ssh-ed25519 invalid").unwrap();
        assert!(!ssh_ready(&key));
        std::fs::write(&public, format!("{}extra line", String::from_utf8_lossy(&saved))).unwrap();
        assert!(!ssh_ready(&key));
        std::fs::write(&public, saved).unwrap();
        assert!(ssh_ready(&key));
    }

    #[test]
    fn readiness_is_profile_specific_and_cancellable_across_many_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("identity");
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        let credential = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
        std::fs::write(credential.path(), "synthetic-credential").unwrap();
        let settings = serde_json::json!({
            "runpod_key_file": credential.path(), "ssh_identity_file": key,
            "docker_config": temp.path().join("docker.json"), "cpu_flavors": [], "gpu_types": [],
            "openai_api_key_file": credential.path(), "anthropic_api_key_file": temp.path().join("missing")
        });
        std::fs::write(temp.path().join("settings.json"), settings.to_string()).unwrap();
        let mut config = CloudConfig::parse(CONFIG).unwrap();
        config.profiles.get_mut("dev").unwrap().capabilities.agents = [horizon_cloud::Agent::Codex].into();
        let profile = config.profiles["dev"].clone();
        for index in 0..100 {
            config.profiles.insert(format!("profile-{index:03}"), profile.clone());
        }
        let mut other = profile;
        other.capabilities.agents = [horizon_cloud::Agent::Claude].into();
        config.profiles.insert("needs-repair".into(), other);
        let cancel = super::super::super::Cancellation::default();
        let ready = ready_profiles(temp.path(), &config, &cancel);
        assert_eq!(ready.len(), 101);
        assert!(!ready.iter().any(|name| name == "needs-repair"));
        cancel.cancel();
        assert!(ready_profiles(temp.path(), &config, &cancel).is_empty());
    }

    #[test]
    fn a_hetzner_only_machine_is_ready_for_the_profiles_hetzner_can_run() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("identity");
        assert!(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&key)
                .status()
                .unwrap()
                .success()
        );
        let token = tempfile::NamedTempFile::new_in(temp.path()).unwrap();
        std::fs::write(token.path(), "synthetic-token").unwrap();
        let settings = |hetzner: serde_json::Value| {
            let settings = serde_json::json!({
                "runpod_key_file": temp.path().join("no-runpod-key"), "ssh_identity_file": key,
                "docker_config": temp.path().join("docker.json"), "cpu_flavors": [], "gpu_types": [],
                "hetzner": hetzner
            });
            std::fs::write(temp.path().join("settings.json"), settings.to_string()).unwrap();
        };
        let mut config = CloudConfig::parse(CONFIG).unwrap();
        let mut gpu = config.profiles["dev"].clone();
        gpu.gpu = true;
        config.profiles.insert("gpu".into(), gpu);
        let cancel = super::super::super::Cancellation::default();
        // No provider at all: nothing is ready.
        settings(serde_json::Value::Null);
        assert!(ready_profiles(temp.path(), &config, &cancel).is_empty());
        settings(serde_json::json!({"token_file": token.path(), "server_types": ["cx33"], "locations": ["hel1"]}));
        // Hetzner runs CPU clouds only.
        assert_eq!(ready_profiles(temp.path(), &config, &cancel), ["dev"]);
    }

    #[test]
    fn discovers_nested_directory_and_linked_checkout_without_switching_branches() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let nested = temp.path().join("sub directory");
        std::fs::create_dir(&nested).unwrap();
        let base = read(&nested).unwrap();
        assert_eq!(base.repository, temp.path().canonicalize().unwrap());
        assert_eq!(base.config.default, "dev");
        let linked = tempfile::tempdir().unwrap();
        let checkout = linked.path().join("linked checkout");
        git(
            temp.path(),
            &["worktree", "add", "--detach", checkout.to_str().unwrap(), "HEAD"],
        );
        let prepared = read(&checkout).unwrap();
        assert_eq!(prepared.repository, checkout.canonicalize().unwrap());
        assert_eq!(prepared.revision, base.revision);
    }
    #[test]
    fn missing_and_invalid_configuration_are_distinct_and_never_echo_yaml() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let path = temp.path().join(".horizon/cloud.yml");
        std::fs::remove_file(&path).unwrap();
        git(temp.path(), &["add", "-u"]);
        commit(temp.path());
        assert!(
            read(temp.path())
                .err()
                .unwrap()
                .to_string()
                .contains("no readable .horizon/cloud.yml")
        );
        std::fs::write(path, "private-invalid-marker").unwrap();
        git(temp.path(), &["add", "."]);
        commit(temp.path());
        let error = read(temp.path()).err().unwrap().to_string();
        assert!(error.contains("Invalid .horizon/cloud.yml"));
        assert!(!error.contains("private-invalid-marker"));
        // A read that fails for another reason is not reported as a missing file.
        std::fs::write(temp.path().join(".horizon/cloud.yml"), "#".repeat(5 * 1024 * 1024)).unwrap();
        git(temp.path(), &["add", "."]);
        commit(temp.path());
        let error = read(temp.path()).err().unwrap().to_string();
        assert!(error.contains("exceeded its bound"), "{error}");
    }
    fn commit(path: &Path) {
        git(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "Update fixture",
            ],
        );
    }
    #[test]
    fn configuration_follows_the_selected_commit_despite_dirty_index_and_worktree() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let original = read(temp.path()).unwrap();
        let path = temp.path().join(".horizon/cloud.yml");
        std::fs::write(&path, CONFIG.replace("cpu: 4", "cpu: 8")).unwrap();
        git(temp.path(), &["add", "."]);
        assert_eq!(read(temp.path()).unwrap().config.profiles["dev"].cpu, 4);
        commit(temp.path());
        assert_eq!(read(temp.path()).unwrap().config.profiles["dev"].cpu, 8);
        let historical = read_revision(temp.path(), &original.revision).unwrap();
        assert_eq!(historical.revision, original.revision);
        assert_eq!(historical.config.profiles["dev"].cpu, 4);
        std::fs::remove_file(&path).unwrap();
        git(temp.path(), &["add", "-u"]);
        assert_eq!(read(temp.path()).unwrap().config.profiles["dev"].cpu, 8);
        std::fs::write(&path, "uncommitted-invalid-marker").unwrap();
        assert_eq!(read(temp.path()).unwrap().config.profiles["dev"].cpu, 8);
    }
    #[test]
    fn committed_configuration_is_not_emitted_to_progress_logs() {
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        let events = std::cell::RefCell::new(Vec::new());
        let cancel = super::super::super::Cancellation::default();
        let emit = |event| events.borrow_mut().push(format!("{event:?}"));
        prepare(
            temp.path().to_str().unwrap(),
            "HEAD",
            &Runner {
                cancel: &cancel,
                emit: &emit,
                secrets: Vec::new(),
            },
        )
        .unwrap();
        assert!(!events.borrow().join("\n").contains("example.invalid/worker"));
    }
    #[test]
    fn cancellation_and_empty_workspace_do_not_prepare_a_launch() {
        let cancel = super::super::super::Cancellation::default();
        cancel.cancel();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        assert!(prepare("", "HEAD", &runner).is_err());
        let temp = tempfile::tempdir().unwrap();
        repository(temp.path());
        assert!(prepare(temp.path().to_str().unwrap(), "HEAD", &runner).is_err());
    }
}
