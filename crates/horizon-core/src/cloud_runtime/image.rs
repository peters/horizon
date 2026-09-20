//! Local Docker/BuildKit image preparation, kept outside horizon-cloud.
mod contract;
use super::{Error, Event, Result, Stage, command::Runner};
use horizon_cloud::{Profile, valid_image};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
const TIMEOUT: Duration = Duration::from_mins(30);
pub struct Images<'a> {
    pub docker_host: Option<&'a str>,
    pub docker_config: &'a Path,
    pub runner: &'a Runner<'a>,
}
impl Images<'_> {
    fn docker(&self) -> Command {
        let mut cmd = Command::new("docker");
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
        profile
            .validate(false)
            .map_err(|_| Error::Invalid("Invalid cloud profile"))?;
        if !horizon_cloud::valid_id(operation_id) {
            return Err(Error::Invalid("Invalid image operation identity"));
        }
        let image = if profile.build.is_some() {
            format!("{}:horizon-{operation_id}", repository_name(&profile.image))
        } else {
            profile.image.clone()
        };
        let mut validated_id = None;
        if let Some(build) = &profile.build {
            (self.runner.emit)(Event::stage(Stage::Build));
            let context = contained(source, &build.context)?;
            let dockerfile = contained(source, &build.dockerfile)?;
            self.runner.run(
                "image build",
                self.docker()
                    .args([
                        "buildx",
                        "build",
                        "--load",
                        "--provenance=false",
                        "--progress=plain",
                        "--platform",
                        &build.platform,
                        "--build-arg",
                        &format!("HORIZON_AGENTS={}", profile.capabilities.agents_argument()),
                        "--build-arg",
                        &format!("HORIZON_BROWSERS={}", profile.capabilities.browsers_argument()),
                        "--build-arg",
                        &format!("HORIZON_DESKTOP={}", profile.capabilities.desktop),
                        "--tag",
                        &image,
                        "--file",
                    ])
                    .arg(dockerfile)
                    .arg(context),
                TIMEOUT,
            )?;
            let image_id = self.runner.run(
                "local image identity",
                self.docker().args(["image", "inspect", "--format", "{{.Id}}", &image]),
                Duration::from_secs(30),
            )?;
            self.validate(image_id.trim(), operation_id, &profile.capabilities)?;
            validated_id = Some(image_id.trim().to_owned());
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
            self.validate(&digest, operation_id, &profile.capabilities)?;
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

fn repository_name(image: &str) -> &str {
    let name = image.split('@').next().unwrap_or(image);
    let slash = name.rfind('/').map_or(0, |i| i + 1);
    name[slash..].find(':').map_or(name, |i| &name[..slash + i])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uploaded_identity_supports_both_docker_stores_and_rejects_changed_tags() {
        let manifest = serde_json::json!({"config": {"digest": "sha256:config"}});
        let reference = "example/worker@sha256:manifest";
        assert!(uploaded_image_matches("sha256:config", reference, &manifest));
        assert!(uploaded_image_matches("sha256:manifest", reference, &manifest));
        assert!(!uploaded_image_matches("sha256:other", reference, &manifest));
    }
}
