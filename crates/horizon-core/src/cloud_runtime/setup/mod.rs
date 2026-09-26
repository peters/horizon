//! First-use machine settings. Secret values never enter repository configuration.
mod storage;
#[cfg(test)]
mod tests;

use super::{
    Error, Result,
    settings::{self, Settings},
};
pub use horizon_cloud::Agent;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Authentication {
    ApiKey,
    Subscription,
}

/// Editable credentials are deliberately neither serializable nor debug-printable.
#[derive(Clone)]
pub struct Draft {
    root: PathBuf,
    original: Option<Vec<u8>>,
    profile_agents: Option<Vec<Agent>>,
    pub settings: Settings,
    pub runpod_key: Zeroizing<String>,
    pub openai_key: Zeroizing<String>,
    pub anthropic_key: Zeroizing<String>,
    pub openai_auth: Authentication,
    pub anthropic_auth: Authentication,
    pub registries: Vec<super::registry::draft::Draft>,
}

impl Draft {
    #[must_use]
    pub fn has_saved_settings(&self) -> bool {
        self.original.is_some()
    }

    /// # Errors
    /// Existing malformed settings must be repaired, never silently replaced.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("settings.json");
        let original = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let settings = original.as_ref().map_or_else(
            || Ok(defaults(root)),
            |bytes| serde_json::from_slice(bytes).map_err(|_| Error::Json),
        )?;
        settings.validate()?;
        Ok(Self {
            root: root.into(),
            original,
            profile_agents: None,
            openai_auth: authentication(settings.openai_api_key_file.as_ref()),
            anthropic_auth: authentication(settings.anthropic_api_key_file.as_ref()),
            registries: settings.registries.as_ref().map_or_else(Vec::new, |config| {
                config
                    .bindings
                    .iter()
                    .map(super::registry::draft::Draft::from_binding)
                    .collect()
            }),
            settings,
            runpod_key: Zeroizing::new(String::new()),
            openai_key: Zeroizing::new(String::new()),
            anthropic_key: Zeroizing::new(String::new()),
        })
    }

    /// Limit this repair to repository capabilities without replacing machine defaults.
    pub fn select_profile_agents(&mut self, agents: Vec<Agent>) {
        self.profile_agents = Some(agents);
    }

    #[must_use]
    pub fn selected_agents(&self) -> &[Agent] {
        self.profile_agents.as_deref().unwrap_or(&self.settings.default_agents)
    }

    /// # Errors
    /// Requires compute access and credentials only for selected API-authenticated agents.
    pub fn validate(&self) -> Result<()> {
        self.settings.validate()?;
        for registry in &self.registries {
            registry.validate()?;
        }
        if self.runpod_key.is_empty() && !self.settings.runpod_key_file.is_file() {
            return Err(Error::Invalid("Enter your RunPod API key"));
        }
        validate_input(&self.runpod_key, Some(&self.settings.runpod_key_file))?;
        if self.runpod_key.is_empty() {
            self.settings.credential()?;
        } else {
            horizon_cloud::Credential::new(self.runpod_key.trim().to_owned())?;
        }
        if self.profile_agents.is_none() && self.settings.default_agents.is_empty() {
            return Err(Error::Invalid("Choose at least one coding agent"));
        }
        for (agent, mode, value, saved) in [
            (
                Agent::Codex,
                self.openai_auth,
                &self.openai_key,
                self.settings.openai_api_key_file.as_ref(),
            ),
            (
                Agent::Claude,
                self.anthropic_auth,
                &self.anthropic_key,
                self.settings.anthropic_api_key_file.as_ref(),
            ),
        ] {
            if !value.is_empty() || (self.selected_agents().contains(&agent) && mode == Authentication::ApiKey) {
                validate_input(value, saved)?;
            }
        }
        Ok(())
    }

    /// # Errors
    /// Saves private bindings atomically and creates a dedicated SSH key on first use.
    /// Caller must run this off the UI thread; it may invoke local ssh-keygen.
    pub fn save(mut self) -> Result<Settings> {
        self.validate()?;
        let mut write = storage::Transaction::new(&self.root)?;
        write.verify_current(self.original.as_deref())?;
        if !self.runpod_key.trim().is_empty() {
            self.settings.runpod_key_file = write.secret("compute", &self.runpod_key)?;
        }
        for (mode, value, binding, name) in [
            (
                self.openai_auth,
                &self.openai_key,
                &mut self.settings.openai_api_key_file,
                "openai",
            ),
            (
                self.anthropic_auth,
                &self.anthropic_key,
                &mut self.settings.anthropic_api_key_file,
                "anthropic",
            ),
        ] {
            if mode == Authentication::Subscription {
                *binding = None;
            } else if !value.trim().is_empty() {
                *binding = Some(write.secret(name, value)?);
            }
        }
        if !self.settings.ssh_identity_file.exists() && self.settings.ssh_identity_file == default_identity(&self.root)
        {
            self.settings.ssh_identity_file = write.ssh_identity()?;
        }
        settings::validate_ssh_identity(&self.settings.ssh_identity_file)?;
        if !self.registries.is_empty() {
            self.settings.registries = Some(super::registry::Config {
                root: self
                    .settings
                    .registries
                    .as_ref()
                    .map_or_else(|| self.root.join("registry"), |config| config.root.clone()),
                bindings: self
                    .registries
                    .iter()
                    .map(|draft| draft.save(|name, value| write.secret(name, value)))
                    .collect::<Result<Vec<_>>>()?,
            });
        }
        write.commit(&self.settings, self.original.as_deref())?;
        Ok(self.settings)
    }
}

fn authentication(binding: Option<&PathBuf>) -> Authentication {
    if binding.is_some() {
        Authentication::ApiKey
    } else {
        Authentication::Subscription
    }
}

fn validate_input(value: &str, saved: Option<&PathBuf>) -> Result<()> {
    if value.is_empty() {
        return settings::validate_private_key_file(
            saved.ok_or(Error::Invalid("Enter an API key or choose subscription login"))?,
        );
    }
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::Invalid("Enter a nonempty API key on one line"));
    }
    Ok(())
}

fn default_identity(root: &Path) -> PathBuf {
    root.join("worker_ed25519")
}

fn defaults(root: &Path) -> Settings {
    Settings {
        default_agents: settings::default_agents(),
        runpod_key_file: root.join("credentials/compute"),
        ssh_identity_file: default_identity(root),
        docker_config: root.join("docker"),
        docker_host: None,
        registry_pull_auth_id: None,
        registries: None,
        cpu_flavors: vec!["cpu3c".into()],
        gpu_types: vec!["NVIDIA RTX A6000".into()],
        data_centers: Vec::new(),
        anthropic_api_key_file: None,
        openai_api_key_file: None,
        anthropic_workspace_id: None,
        git_credentials: Vec::new(),
        browserstack_credentials: Vec::new(),
        hetzner: None,
        placement: None,
    }
}
