//! Machine-local image grants. Publishing and worker pulling never share a binding.
pub use horizon_cloud::runpod::registry::State;
mod credentials;
mod management;
pub use management::{Action, Status, Validation, manage};
pub mod draft;
mod store;
#[cfg(test)]
mod tests;

use super::{Cancellation, Error, Result, command::Runner, settings::Settings};
use credentials::Material;
use horizon_cloud::runpod::{RunPod, registry::PullBinding};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use store::Journal;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub root: PathBuf,
    pub bindings: Vec<Binding>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Exact registry hostname and repository; neither a prefix nor a tag.
    pub repository: String,
    pub publish: Option<Auth>,
    pub pull: Auth,
    pub read_only_confirmed: bool,
    pub generation: String,
    #[serde(default)]
    pub retired: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    pub username: String,
    pub secret_file: PathBuf,
    /// RFC3339 UTC timestamp; absent means the issuer's expiry is unknown.
    pub expires_at: Option<String>,
}

impl Config {
    /// # Errors
    /// Rejects ambiguous grants and non-local storage before reading any secret.
    pub fn validate(&self) -> Result<()> {
        if !self.root.is_absolute() {
            return Err(Error::Invalid("Registry state must use an absolute private path"));
        }
        let mut repositories = std::collections::BTreeSet::new();
        let mut generations = std::collections::BTreeSet::new();
        for binding in &self.bindings {
            binding.validate()?;
            for generation in std::iter::once(&binding.generation).chain(&binding.retired) {
                if !generations.insert(generation) {
                    return Err(Error::Invalid("Registry generations must be globally unique"));
                }
            }
            let (_, path) = binding
                .repository
                .split_once('/')
                .ok_or(Error::Invalid("Use an explicit registry hostname and repository"))?;
            let key = (credentials::registry_authority(&binding.repository), path);
            if !repositories.insert(key) {
                return Err(Error::Invalid("Duplicate registry repository binding"));
            }
        }
        Ok(())
    }

    fn select(&self, image: &str) -> Result<Option<&Binding>> {
        self.validate()?;
        let first = image.split('/').next().unwrap_or_default();
        if horizon_cloud::valid_image(image)
            && (!image.contains('/') || (!first.contains('.') && !first.contains(':') && first != "localhost"))
        {
            if self
                .bindings
                .iter()
                .any(|binding| credentials::registry_authority(&binding.repository) == "docker.io")
            {
                return Err(Error::Invalid(
                    "Use an explicit docker.io repository when Docker Hub bindings are configured",
                ));
            }
            return Ok(None);
        }
        let repository = repository(image)?;
        if let Some(binding) = self.bindings.iter().find(|binding| binding.repository == repository) {
            return Ok(Some(binding));
        }
        let host = credentials::registry_authority(repository);
        if self
            .bindings
            .iter()
            .any(|binding| credentials::registry_authority(&binding.repository) == host)
        {
            return Err(Error::Invalid(
                "Image repository does not match the authorized registry scope",
            ));
        }
        Ok(None)
    }
}

impl Binding {
    fn validate(&self) -> Result<()> {
        if repository(&self.repository)? != self.repository || !self.repository.contains('/') {
            return Err(Error::Invalid(
                "Use an exact registry/repository without a tag or digest",
            ));
        }
        for generation in std::iter::once(&self.generation).chain(&self.retired) {
            if !horizon_cloud::valid_id(generation) || generation.len() > 64 {
                return Err(Error::Invalid("Invalid registry generation"));
            }
        }
        if self.retired.contains(&self.generation) {
            return Err(Error::Invalid("Current registry generation cannot be retired"));
        }
        for auth in std::iter::once(&self.pull).chain(self.publish.as_ref()) {
            auth.validate()?;
        }
        Ok(())
    }
}

impl Auth {
    pub(super) fn check_expiry(&self) -> Result<()> {
        if let Some(expiry) = &self.expires_at
            && parse_expiry(expiry)? <= time::OffsetDateTime::now_utc().unix_timestamp()
        {
            return Err(Error::Invalid(
                "Registry credential has expired; replace it before deploying",
            ));
        }
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        if !self.secret_file.is_absolute()
            || self.username.is_empty()
            || self.username.len() > 256
            || self
                .username
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || c == ':')
        {
            return Err(Error::Invalid(
                "Registry login needs a username and an absolute private secret-file path",
            ));
        }
        if let Some(expiry) = &self.expires_at {
            parse_expiry(expiry)?;
        }
        Ok(())
    }
}

/// Retains the generation lock through allocation, excluding simultaneous revocation.
pub struct Prepared {
    binding: Binding,
    publish: Option<Material>,
    pull: Material,
    journal: Journal,
    verified: bool,
    docker_host: Option<String>,
}

impl Prepared {
    /// # Errors
    /// Validates explicit grants, expiry, secret separation and source isolation before a build.
    pub fn for_image(settings: &Settings, image: &str, source: Option<&Path>, building: bool) -> Result<Option<Self>> {
        let Some(config) = &settings.registries else {
            return Ok(None);
        };
        let Some(binding) = config.select(image)? else {
            return Ok(None);
        };
        if !binding.read_only_confirmed {
            return Err(Error::Invalid(
                "Confirm that the worker pull credential is repository-scoped and read-only",
            ));
        }
        let source = source.map(Path::canonicalize).transpose()?;
        let pull = Material::load(&binding.pull, &binding.repository, source.as_deref())?;
        credentials::ensure_separate(&binding.pull, &pull, binding.publish.as_ref(), source.as_deref())?;
        let publish = binding
            .publish
            .as_ref()
            .filter(|_| building)
            .map(|auth| Material::load(auth, &binding.repository, source.as_deref()))
            .transpose()?;
        if building && publish.is_none() {
            return Err(Error::Invalid("Image publishing needs a separate push credential"));
        }
        if publish
            .as_ref()
            .is_some_and(|push| push.fingerprint == pull.fingerprint)
        {
            return Err(Error::Invalid(
                "Publishing and worker pulling must use different credentials",
            ));
        }
        if let Some(source) = &source {
            store::ensure_outside_source(&config.root, source)?;
        }
        let login_identity =
            credentials::fingerprint(format!("{}\0{}", binding.pull.username, pull.fingerprint).as_bytes());
        let journal = Journal::open(config, binding, settings, Some(&login_identity))?;
        if matches!(journal.state(), State::Revoking(_) | State::Revoked) {
            return Err(Error::Invalid(
                "Worker pull binding is revoked; rotate it before deploying",
            ));
        }
        Ok(Some(Self {
            binding: binding.clone(),
            publish,
            pull,
            journal,
            verified: false,
            docker_host: settings.docker_host.clone(),
        }))
    }

    #[must_use]
    pub fn docker_config(&self, building: bool) -> &Path {
        if building {
            self.publish
                .as_ref()
                .map_or(self.pull.config.path(), |publish| publish.config.path())
        } else {
            self.pull.config.path()
        }
    }

    /// # Errors
    /// Validates issuer-reported pull scope and the exact immutable manifest using only the pull login.
    pub fn verify_image(&mut self, image: &str, cancel: &Cancellation) -> Result<()> {
        self.verify_image_with(image, cancel, Command::new("docker"))
    }

    fn verify_image_with(&mut self, image: &str, cancel: &Cancellation, mut command: Command) -> Result<()> {
        self.verified = false;
        command.env_remove("DOCKER_AUTH_CONFIG");
        self.binding.pull.check_expiry()?;
        if repository(image)? != self.binding.repository || !image.contains("@sha256:") {
            return Err(Error::Invalid(
                "Pull validation requires an immutable image in the authorized repository",
            ));
        }
        let observed_expiry = self.pull.verify_scope(&self.binding.repository, cancel)?;
        let runner = Runner {
            cancel,
            emit: &|_| {},
            secrets: self.pull.redactions(),
        };
        if let Some(host) = &self.docker_host {
            command.arg("--host").arg(host);
        }
        let output = runner.run("Registry pull validation", command
            .arg("--config").arg(self.pull.config.path())
            .args(["buildx", "imagetools", "inspect", image, "--format", "{{json .Manifest}}"]), Duration::from_secs(60))
            .map_err(|error| match error {
                Error::Provider(horizon_cloud::CloudError::Cancelled) => error,
                _ => Error::Invalid("Worker pull credential cannot read the immutable image; check expiry, scope and image availability"),
            })?;
        let manifest: serde_json::Value = serde_json::from_str(&output).map_err(|_| Error::Json)?;
        if image.split_once('@').map(|(_, digest)| digest) != manifest["digest"].as_str() {
            return Err(Error::Invalid("Registry returned a different image digest"));
        }
        cancel.check()?;
        self.journal.record_validation(Validation {
            image: image.into(),
            checked_at: time::OffsetDateTime::now_utc().unix_timestamp(),
            scope: if credentials::is_github_registry(&self.binding.repository) {
                "issuer-verified read:packages"
            } else {
                "owner-confirmed read-only; issuer scope introspection unavailable"
            }
            .into(),
            expires_at: observed_expiry.or_else(|| self.binding.pull.expires_at.clone()),
        })?;
        self.verified = true;
        Ok(())
    }

    /// # Errors
    /// Sends only the verified pull secret, with durable reconciliation after uncertain responses.
    pub fn ensure_provider(&mut self, provider: &RunPod, cancel: &Cancellation) -> Result<String> {
        if !self.verified {
            return Err(Error::Invalid(
                "Validate immutable image access before transferring a pull credential",
            ));
        }
        self.binding.pull.check_expiry()?;
        let credential = horizon_cloud::Credential::new(self.pull.secret.to_string())?;
        let input = PullBinding {
            operation_id: &self.binding.generation,
            username: &self.binding.pull.username,
            credential: &credential,
        };
        let mut state = self.journal.state().clone();
        let binding = provider.ensure_registry_binding(&input, &mut state, cancel, |next| self.journal.save(next))?;
        Ok(binding.id)
    }

    #[must_use]
    pub fn redactions(&self) -> Vec<String> {
        let mut values = self.pull.redactions();
        if let Some(publish) = &self.publish {
            values.extend(publish.redactions());
        }
        values
    }
}

/// # Errors
/// Revokes a selected current/retired generation without transmitting a secret or changing workers.
pub fn revoke(settings: &Settings, repository: &str, generation: &str, cancel: &Cancellation) -> Result<()> {
    let (config, binding) = find(settings, repository, generation)?;
    let mut journal = Journal::open_generation(config, binding, generation, settings)?;
    let mut state = journal.state().clone();
    RunPod::new(settings.credential()?)
        .revoke_registry_binding(generation, &mut state, cancel, |next| journal.save(next))?;
    Ok(())
}

/// # Errors
/// Reconciles only recorded provider operations; does not allocate compute or publish images.
pub fn reconcile(settings: &Settings, repository: &str, generation: &str, cancel: &Cancellation) -> Result<State> {
    let (config, binding) = find(settings, repository, generation)?;
    let mut journal = Journal::open_generation(config, binding, generation, settings)?;
    let mut state = journal.state().clone();
    RunPod::new(settings.credential()?)
        .reconcile_registry_binding(generation, &mut state, cancel, |next| journal.save(next))?;
    Ok(state)
}

fn find<'a>(settings: &'a Settings, repository: &str, generation: &str) -> Result<(&'a Config, &'a Binding)> {
    let config = settings
        .registries
        .as_ref()
        .ok_or(Error::Invalid("No registry bindings configured"))?;
    let binding = config
        .select(repository)?
        .ok_or(Error::Invalid("Registry repository is not bound"))?;
    if binding.generation != generation && !binding.retired.iter().any(|value| value == generation) {
        return Err(Error::Invalid("Registry generation does not belong to this repository"));
    }
    Ok((config, binding))
}

pub(super) fn repository(image: &str) -> Result<&str> {
    if !horizon_cloud::valid_image(image) {
        return Err(Error::Invalid("Invalid registry image reference"));
    }
    let image = image.split('@').next().unwrap_or(image);
    let slash = image
        .rfind('/')
        .ok_or(Error::Invalid("Use an explicit registry hostname and repository"))?;
    let image = image[slash..].find(':').map_or(image, |colon| &image[..slash + colon]);
    if image
        .split('/')
        .skip(1)
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::Invalid(
            "Registry repository path must contain nonempty components",
        ));
    }
    let host = image.split('/').next().unwrap_or_default();
    if !host.contains('.') && !host.contains(':') && host != "localhost" {
        return Err(Error::Invalid("Use an explicit registry hostname"));
    }
    Ok(image)
}

pub(super) fn parse_expiry(value: &str) -> Result<i64> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map(time::OffsetDateTime::unix_timestamp)
        .map_err(|_| Error::Invalid("Registry expiry must use RFC3339, for example 2027-01-01T00:00:00Z"))
}
