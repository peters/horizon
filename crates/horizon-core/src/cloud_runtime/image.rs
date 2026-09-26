//! Local Docker/BuildKit image preparation, kept outside horizon-cloud.
pub mod agents;
mod contract;
mod layers;
use super::{Error, Event, Result, Stage, command::Runner, progress::Progress};
use horizon_cloud::{Capabilities, Profile, valid_image};
pub use layers::Layer;
use layers::{Builder, Recipe};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
const TIMEOUT: Duration = Duration::from_mins(30);
pub struct Images<'a> {
    pub docker_host: Option<&'a str>,
    pub isolated_registry: bool,
    pub docker_config: &'a Path,
    pub runner: &'a Runner<'a>,
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
        let image = if profile.build.is_some() {
            format!("{}:{tag}", repository_name(&profile.image))
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
            let arguments = build_arguments(&profile.capabilities, &releases);
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
            self.runner.transfer(
                "Uploading image",
                self.docker().args(["push", &image]),
                super::command::terminal_progress::Transfer::Image,
                TIMEOUT,
            )?;
        }
        let digest = self.resolve(&image)?;
        if let Some(image_id) = validated_id {
            let manifest = self.runner.run(
                "pushed image identity",
                self.docker()
                    .args(["buildx", "imagetools", "inspect", &digest, "--raw"]),
                Duration::from_secs(60),
            )?;
            let manifest: serde_json::Value =
                serde_json::from_str(&manifest).map_err(|_| Error::Invalid("Invalid pushed manifest"))?;
            if !uploaded_image_matches(&image_id, &digest, &manifest) {
                return Err(Error::Invalid(
                    "Pushed image differs from the verified local worker image",
                ));
            }
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
/// The tag `prepare` publishes a cloud's first image under.
#[must_use]
pub fn default_tag(operation_id: &str) -> String {
    format!("horizon-{operation_id}")
}
/// Capability selections plus the exact agent releases that key each install layer.
fn build_arguments(capabilities: &Capabilities, releases: &agents::Releases) -> Vec<String> {
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
