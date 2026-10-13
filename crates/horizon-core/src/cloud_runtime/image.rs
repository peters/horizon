//! Local Docker/BuildKit image preparation, kept outside horizon-cloud. The pinned
//! quick-start image needs none of it (`quick_start::trusted_contract`).
pub mod agents;
mod contract;
mod layers;
#[cfg(all(test, unix))]
pub(super) mod without_docker;
use super::{Error, Event, Result, Stage, command::Runner, progress::Progress, repository::launch::quick_start};
use horizon_cloud::{Capabilities, Profile, valid_image};
pub use layers::Layer;
use layers::{Builder, Recipe};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
const TIMEOUT: Duration = Duration::from_mins(30);
const SETTINGS_LABEL: &str = "io.horizon.worker.settings";

/// The deployment stages that preparing `profile` never runs: an image-only profile has
/// nothing to build or push, so [`Images::prepare_layered`] reports neither stage.
#[must_use]
pub fn skipped_stages(profile: &Profile) -> &'static [Stage] {
    if profile.build.is_none() {
        &[Stage::Build, Stage::Push]
    } else {
        &[]
    }
}

pub struct Images<'a> {
    pub docker_host: Option<&'a str>,
    pub isolated_registry: bool,
    pub docker_config: &'a Path,
    pub runner: &'a Runner<'a>,
    /// Logs the Docker configuration in to `ghcr.io` as the person before a push there;
    /// `None` pushes with the configuration as it is.
    pub publisher: Option<super::github::publish::Publisher<'a>>,
}
impl Images<'_> {
    fn docker(&self) -> Command {
        let mut cmd = Command::new("docker");
        if self.isolated_registry {
            cmd.env_remove("DOCKER_AUTH_CONFIG");
        }
        cmd.arg("--config").arg(self.docker_config);
        if let Some(host) = self.docker_host {
            cmd.arg("--host").arg(host);
        }
        cmd
    }
    /// # Errors
    /// Builds and checks locally, then pushes and resolves the registry digest.
    /// Never performs a provider allocation. `BuildKit` handles cache/.dockerignore.
    /// The pinned quick-start image is already a digest with a known contract, so it
    /// uses no Docker at all.
    pub fn prepare(&self, profile: &Profile, source: &Path, operation_id: &str) -> Result<String> {
        self.prepare_tagged(profile, source, operation_id, &default_tag(operation_id))
    }
    /// # Errors
    /// As `prepare`, publishing a built image under `tag`. The contract check keeps
    /// its container named by `operation_id`, which recovers an interrupted check.
    pub fn prepare_tagged(&self, profile: &Profile, source: &Path, operation_id: &str, tag: &str) -> Result<String> {
        self.prepare_layered(profile, source, &[], operation_id, tag, false)
    }
    /// # Errors
    /// As `prepare_tagged`, building each sibling layer on the image before it. The primary
    /// recipe and every layer but the last get local tags that are never pushed and are
    /// removed afterwards. Only the final image is checked, additionally for sibling
    /// checkouts and, with `grants`, for version 2 Git grants, and only it is pushed.
    /// Without layers `grants` is ignored and the build is exactly `prepare_tagged`'s. Every
    /// layer must build for the primary's platform.
    pub fn prepare_layered(
        &self,
        profile: &Profile,
        source: &Path,
        layers: &[Layer<'_>],
        operation_id: &str,
        tag: &str,
        grants: bool,
    ) -> Result<String> {
        profile
            .validate(false)
            .map_err(|_| Error::Invalid("Invalid cloud profile"))?;
        if !horizon_cloud::valid_id(operation_id) {
            return Err(Error::Invalid("Invalid image operation identity"));
        }
        if !valid_tag(tag) {
            return Err(Error::Invalid("Invalid image tag"));
        }
        if !layers.is_empty() && profile.build.is_none() {
            return Err(super::siblings::SiblingError::PrimaryImageOnly.into());
        }
        if profile.build.is_none() && quick_start::trusted_contract(&profile.image, &profile.capabilities).is_some() {
            (self.runner.emit)(Event::Output(TRUSTED.into()));
            self.validate(&profile.image, operation_id, profile)?;
            return Ok(profile.image.clone());
        }
        require_docker()?;
        let image = if profile.build.is_some() {
            built_reference(profile, tag)
        } else {
            profile.image.clone()
        };
        let mut validated_id = None;
        if let Some(build) = &profile.build {
            (self.runner.emit)(Event::stage(Stage::Build));
            let primary = Recipe::new(source, build)?;
            let layers = layers
                .iter()
                .map(|layer| {
                    if layer.build.platform != build.platform {
                        return Err(super::siblings::SiblingError::Platform {
                            alias: layer.alias.to_owned(),
                            sibling: layer.build.platform.clone(),
                            primary: build.platform.clone(),
                        }
                        .into());
                    }
                    Ok((layer.alias, Recipe::new(layer.source, layer.build)?))
                })
                .collect::<Result<Vec<_>>>()?;
            (self.runner.emit)(Event::Progress(Progress::activity(
                "Looking up the latest agent CLI releases",
            )));
            let releases = agents::latest(self.runner.cancel)?;
            (self.runner.emit)(Event::Output(format!("Agent CLI releases: {}", releases.summary())));
            let mut arguments = build_arguments(&profile.capabilities, &releases);
            arguments.extend(settings_label(profile));
            let builder = Builder {
                docker: || self.docker(),
                runner: self.runner,
            };
            let image_id = if layers.is_empty() {
                builder.build(&image, &primary, &arguments)?;
                let image_id = builder.image_id(&image)?;
                self.validate(&image_id, operation_id, profile)?;
                image_id
            } else {
                let image_id = builder.build_layers(&image, tag, &primary, &layers, &arguments)?;
                self.validate_siblings_contract(&image_id, operation_id, profile, grants)?;
                image_id
            };
            validated_id = Some(image_id);
            (self.runner.emit)(Event::stage(Stage::Push));
            if let Some(publisher) = self.publisher {
                publisher.login(&image, self.runner)?;
            }
            self.runner.transfer(
                "Uploading image",
                self.docker().args(["push", &image]),
                super::command::terminal_progress::Transfer::Image,
                TIMEOUT,
            )?;
        }
        let digest = self.resolve(&image)?;
        if let Some(image_id) = validated_id
            && !self.pushed_as(&image_id, &digest)?
        {
            return Err(Error::Invalid(
                "Pushed image differs from the verified local worker image",
            ));
        }
        if profile.build.is_none() {
            self.runner.transfer(
                "Downloading worker image",
                self.docker().args(["pull", "--platform", "linux/amd64", &digest]),
                super::command::terminal_progress::Transfer::Pull,
                TIMEOUT,
            )?;
            self.validate(&digest, operation_id, profile)?;
        }
        Ok(digest)
    }
    /// Whether the registry image `digest` is the local image `local_id`.
    fn pushed_as(&self, local_id: &str, digest: &str) -> Result<bool> {
        let manifest = self.runner.run(
            "pushed image identity",
            self.docker().args(["buildx", "imagetools", "inspect", digest, "--raw"]),
            Duration::from_secs(60),
        )?;
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest).map_err(|_| Error::Invalid("Invalid pushed manifest"))?;
        Ok(uploaded_image_matches(local_id, digest, &manifest))
    }
    /// # Errors
    /// The unpacked size in bytes of the registry image `digest` that a build of `profile`
    /// under `tag` pushed, as `docker image inspect` reports it for the local image. The
    /// tag can be moved after the push, so the size counts only while the tag still names
    /// the image pushed as `digest`. A profile without a build section has no such image.
    pub fn unpacked_bytes(&self, profile: &Profile, tag: &str, digest: &str) -> Result<u64> {
        if profile.build.is_none() || !valid_tag(tag) {
            return Err(Error::Invalid("Only an image built under a valid tag has a local size"));
        }
        let image = built_reference(profile, tag);
        let output = self
            .runner
            .run_parsed(
                "local image size",
                self.docker()
                    .args(["image", "inspect", "--format", "{{.Id}} {{.Size}}", &image]),
                Duration::from_secs(30),
            )
            .map_err(|error| match error {
                // Docker's own message is in the output; the hint names the way out.
                Error::Command(_) => Error::Invalid(
                    "Local Docker cannot report the size of the built image. If the image is gone, cancel the replacement and rebuild",
                ),
                other => other,
            })?;
        let (local_id, size) = output
            .trim()
            .split_once(' ')
            .ok_or(Error::Invalid("Docker reported no size for the built image"))?;
        if !self.pushed_as(local_id, digest)? {
            return Err(Error::Invalid(
                "The built image's local tag no longer names the image that was pushed; cancel the replacement and rebuild",
            ));
        }
        size.parse()
            .map_err(|_| Error::Invalid("Docker reported no size for the built image"))
    }
    fn resolve(&self, image: &str) -> Result<String> {
        let output = self.runner.run(
            "registry manifest",
            self.docker().args([
                "buildx",
                "imagetools",
                "inspect",
                image,
                "--format",
                "{{json .Manifest}}",
            ]),
            Duration::from_secs(60),
        )?;
        let manifest: serde_json::Value =
            serde_json::from_str(output.trim()).map_err(|_| Error::Invalid("Invalid registry image manifest"))?;
        let digest = manifest["digest"]
            .as_str()
            .ok_or(Error::Invalid("Registry returned no immutable digest"))?;
        let name = image.split('@').next().unwrap_or(image);
        let last_slash = name.rfind('/').map_or(0, |i| i + 1);
        let name = if let Some(colon) = name[last_slash..].find(':') {
            &name[..last_slash + colon]
        } else {
            name
        };
        let reference = format!("{name}@{digest}");
        if !valid_image(&reference) || !reference.contains("@sha256:") {
            return Err(Error::Invalid("Registry digest is invalid"));
        }
        Ok(reference)
    }
}
/// Said instead of the digest lookup, download and local check of the pinned quick-start image.
const TRUSTED: &str = "Worker contract: the pinned quick-start image passed this check when it was published, so it does not run in local Docker";

/// Refused before the first Docker command when no `docker` program is on `PATH`, so the
/// person reads what is missing rather than a failed process start.
const DOCKER_MISSING: &str = "Docker is not installed on this computer or not on its PATH, and this cloud's image needs a local Docker build or check. Install Docker. Quick start on the base image that this Horizon version pins needs none.";

fn require_docker() -> Result<()> {
    let program = if cfg!(windows) { "docker.exe" } else { "docker" };
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|directory| directory.join(program).is_file()))
        .then_some(())
        .ok_or(Error::Invalid(DOCKER_MISSING))
}

/// The tag `prepare` publishes a cloud's first image under.
#[must_use]
pub fn default_tag(operation_id: &str) -> String {
    format!("horizon-{operation_id}")
}
/// Capability selections plus the exact agent releases that key each install layer.
/// The `--build-arg` options deployment passes for `capabilities`: its capability
/// switches and the agent CLI releases, so a setup proof builds the same image.
#[must_use]
pub fn build_arguments(capabilities: &Capabilities, releases: &agents::Releases) -> Vec<String> {
    [
        format!("HORIZON_AGENTS={}", capabilities.agents_argument()),
        format!("HORIZON_BROWSERS={}", capabilities.browsers_argument()),
        format!("HORIZON_DESKTOP={}", capabilities.desktop),
        format!("HORIZON_BROWSERSTACK={}", capabilities.browserstack.is_some()),
    ]
    .into_iter()
    .chain(releases.build_arguments())
    .flat_map(|value| ["--build-arg".to_owned(), value])
    .collect()
}
/// The label that records the profile settings an image was built for: its rebuild
/// settings (`Profile::with_rebuild_settings`), some of which reach the worker only with
/// the image's switch. So a rebuild that changes them makes another image, which the
/// provider tells apart from the worker's current one, while a cached build with the same
/// settings keeps its digest. It holds only committed profile values: the capabilities
/// without hosted devices, the readiness timeout, the idle stop and the build paths.
fn settings_label(profile: &Profile) -> [String; 2] {
    // Hosted devices are fixed at creation, and their names stay out of public metadata.
    let capabilities = Capabilities {
        browserstack: None,
        ..profile.capabilities.clone()
    };
    let settings = serde_json::json!({
        "capabilities": capabilities,
        "readiness_seconds": profile.bootstrap.readiness_seconds,
        "idle_stop_minutes": profile.idle_stop_minutes,
        "build": profile.build,
    });
    ["--label".to_owned(), format!("{SETTINGS_LABEL}={settings}")]
}
fn uploaded_image_matches(local_id: &str, reference: &str, manifest: &serde_json::Value) -> bool {
    // Classic Docker identifies the config; containerd-backed Docker identifies the manifest.
    manifest["config"]["digest"].as_str() == Some(local_id)
        || reference.split_once('@').is_some_and(|(_, digest)| digest == local_id)
}
fn contained(root: &Path, value: &str) -> Result<PathBuf> {
    let root = root.canonicalize()?;
    let path = root.join(value).canonicalize()?;
    if !path.starts_with(root) {
        return Err(Error::Invalid("Build path escapes committed repository"));
    }
    Ok(path)
}

/// Docker's tag grammar: at most 128 word characters, dots and dashes, not led by either.
fn valid_tag(tag: &str) -> bool {
    tag.len() <= 128
        && tag
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

/// The local tag that a build of `profile` under `tag` gets and pushes.
fn built_reference(profile: &Profile, tag: &str) -> String {
    format!("{}:{tag}", repository_name(&profile.image))
}

fn repository_name(image: &str) -> &str {
    let name = image.split('@').next().unwrap_or(image);
    let slash = name.rfind('/').map_or(0, |i| i + 1);
    name[slash..].find(':').map_or(name, |i| &name[..slash + i])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_registry_commands_isolate_auth_and_legacy_commands_preserve_it() {
        let cancel = super::super::Cancellation::default();
        let runner = Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        for isolated_registry in [false, true] {
            let images = Images {
                docker_host: None,
                docker_config: Path::new("/synthetic/config"),
                isolated_registry,
                runner: &runner,
                publisher: None,
            };
            let command = images.docker();
            let override_auth = command.get_envs().find(|(key, _)| *key == "DOCKER_AUTH_CONFIG");
            if isolated_registry {
                assert_eq!(override_auth, Some((std::ffi::OsStr::new("DOCKER_AUTH_CONFIG"), None)));
            } else {
                assert!(override_auth.is_none());
            }
        }
    }

    #[test]
    fn replacement_tags_follow_the_docker_tag_grammar() {
        let tag = format!("horizon-{}-{}", "c".repeat(36), "d".repeat(32));
        assert!(valid_tag(&tag));
        for invalid in ["", "-lead", ".lead", "has space", "has:colon", &"a".repeat(129)] {
            assert!(!valid_tag(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn build_repository_retains_registry_namespace_and_port() {
        for (reference, expected) in [
            ("registry.example.com/team/worker", "registry.example.com/team/worker"),
            (
                "registry.example.com:5000/team/worker:latest",
                "registry.example.com:5000/team/worker",
            ),
            (
                "registry.example.com/team/worker@sha256:abc",
                "registry.example.com/team/worker",
            ),
            ("docker.io/team/worker:latest", "docker.io/team/worker"),
        ] {
            assert_eq!(repository_name(reference), expected);
        }
    }

    #[test]
    fn the_settings_label_changes_with_each_rebuild_setting_and_nothing_else() {
        let profile = horizon_cloud::CloudConfig::parse(horizon_cloud::EXAMPLE)
            .unwrap()
            .profiles["image-only"]
            .clone();
        let label = settings_label(&profile);
        assert_eq!(label[0], "--label");
        assert!(label[1].starts_with("io.horizon.worker.settings={"), "{}", label[1]);
        assert_eq!(
            settings_label(&profile.clone()),
            label,
            "a cached build keeps its digest"
        );
        let mut resized = profile.clone();
        resized.cpu += 1;
        resized.storage.volume_gb += 1;
        // Hosted devices are fixed at creation, and their names stay out of the label.
        resized.capabilities.browserstack = Some(horizon_cloud::BrowserStack {
            provider: "synthetic-devices".into(),
            targets: ["synthetic-phone".into()].into(),
            local_ports: std::collections::BTreeSet::new(),
        });
        assert_eq!(settings_label(&resized), label);
        assert!(!label[1].contains("synthetic"));
        let edits: [fn(&mut Profile); 4] = [
            |profile| profile.capabilities.desktop = !profile.capabilities.desktop,
            |profile| profile.idle_stop_minutes = Some(30),
            |profile| profile.bootstrap.readiness_seconds += 1,
            |profile| {
                profile.build = Some(horizon_cloud::Build {
                    context: ".".into(),
                    dockerfile: "Other".into(),
                    platform: "linux/amd64".into(),
                });
            },
        ];
        for edit in edits {
            let mut changed = profile.clone();
            edit(&mut changed);
            assert_ne!(settings_label(&changed), label, "{changed:?}");
        }
    }

    #[test]
    fn build_arguments_carry_capabilities_and_each_agent_release() {
        let capabilities: Capabilities =
            serde_json::from_str(r#"{"agents":["claude"],"browsers":["firefox"],"desktop":true}"#).unwrap();
        let releases = [
            agents::Release::new(horizon_cloud::Agent::Codex, "0.156.1").unwrap(),
            agents::Release::new(horizon_cloud::Agent::Claude, "2.1.281").unwrap(),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            build_arguments(&capabilities, &releases),
            [
                "--build-arg",
                "HORIZON_AGENTS=claude",
                "--build-arg",
                "HORIZON_BROWSERS=firefox",
                "--build-arg",
                "HORIZON_DESKTOP=true",
                "--build-arg",
                "HORIZON_BROWSERSTACK=false",
                "--build-arg",
                "HORIZON_CODEX_VERSION=0.156.1",
                "--build-arg",
                "HORIZON_CLAUDE_VERSION=2.1.281",
            ]
        );
        assert!(agents::Release::new(horizon_cloud::Agent::Grok, "1.0.0 --tag other").is_err());
    }

    #[test]
    fn uploaded_identity_supports_both_docker_stores_and_rejects_changed_tags() {
        let manifest = serde_json::json!({"config": {"digest": "sha256:config"}});
        let reference = "example/worker@sha256:manifest";
        assert!(uploaded_image_matches("sha256:config", reference, &manifest));
        assert!(uploaded_image_matches("sha256:manifest", reference, &manifest));
        assert!(!uploaded_image_matches("sha256:other", reference, &manifest));
    }
}
