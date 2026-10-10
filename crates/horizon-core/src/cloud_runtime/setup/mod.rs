//! First-use machine settings. Secret values never enter repository configuration.
mod credentials;
mod hetzner;
mod storage;
#[cfg(test)]
mod tests;

use super::{
    Error, Result,
    settings::{self, Settings},
};
pub use credentials::SavedCredentials;
pub use hetzner::Draft as HetznerDraft;
pub use horizon_cloud::Agent;
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Authentication {
    ApiKey,
    Subscription,
    /// Sign in with `ChatGPT`: this machine's saved connection authorizes Codex through
    /// the user's `ChatGPT` plan. Only Codex supports it.
    ChatGpt,
}

/// Editable credentials are deliberately neither serializable nor debug-printable.
#[derive(Clone)]
pub struct Draft {
    root: PathBuf,
    original: Option<Vec<u8>>,
    profile_agents: Option<Vec<Agent>>,
    pub settings: Settings,
    pub saved_credentials: SavedCredentials,
    pub runpod_key: Zeroizing<String>,
    pub openai_key: Zeroizing<String>,
    pub anthropic_key: Zeroizing<String>,
    pub openai_auth: Authentication,
    pub anthropic_auth: Authentication,
    /// This machine's active `ChatGPT` sign-in, if any, without its tokens.
    pub chatgpt: Option<super::chatgpt::Connection>,
    /// A failed status read is distinct from a verified signed-out account.
    pub chatgpt_status_error: Option<String>,
    pub registries: Vec<super::registry::draft::Draft>,
    /// Optional: Hetzner as a second provider for CPU clouds.
    pub hetzner: HetznerDraft,
}

impl Draft {
    /// The cloud root these settings belong to.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

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
        let (chatgpt, chatgpt_status_error) = match super::chatgpt::status(root) {
            Ok(connection) => (connection, None),
            Err(error) => {
                tracing::warn!(%error, "could not read the saved ChatGPT sign-in");
                (None, Some(error.to_string()))
            }
        };
        Ok(Self {
            root: root.into(),
            original,
            profile_agents: None,
            openai_auth: settings.openai_auth.map_or_else(
                || openai_authentication(settings.openai_api_key_file.as_ref()),
                |mode| {
                    // Codex no longer offers the worker-terminal login; a saved
                    // choice of it becomes the ChatGPT sign-in.
                    if mode == Authentication::Subscription {
                        Authentication::ChatGpt
                    } else {
                        mode
                    }
                },
            ),
            anthropic_auth: authentication(settings.anthropic_api_key_file.as_ref()),
            chatgpt,
            chatgpt_status_error,
            registries: settings.registries.as_ref().map_or_else(Vec::new, |config| {
                config
                    .bindings
                    .iter()
                    .map(super::registry::draft::Draft::from_binding)
                    .collect()
            }),
            hetzner: HetznerDraft::from_settings(settings.hetzner.as_ref()),
            saved_credentials: SavedCredentials::load(&settings),
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
    /// A provider, selected agent credential, or saved plan sign-in is missing or invalid.
    pub fn validate(&self) -> Result<()> {
        self.validate_with_chatgpt_gate(true)
    }

    /// # Errors
    /// Requires one compute provider, `RunPod` or Hetzner, and credentials only for
    /// selected API-authenticated agents. With the gate off, a selected Codex on the
    /// `ChatGPT` plan may lack its sign-in: partial saves such as a provider key must
    /// not be blocked by the agent credentials.
    fn validate_with_chatgpt_gate(&self, require_chatgpt_sign_in: bool) -> Result<()> {
        self.settings.validate()?;
        for registry in &self.registries {
            registry.validate()?;
        }
        let runpod = !self.runpod_key.is_empty() || self.settings.runpod_configured();
        if !runpod && !self.hetzner.enabled {
            return Err(Error::Invalid(
                "Enter your RunPod API key, or turn on Hetzner and enter its token",
            ));
        }
        if runpod {
            validate_input(&self.runpod_key, Some(&self.settings.runpod_key_file))?;
            if self.runpod_key.is_empty() {
                self.settings.credential()?;
            } else {
                horizon_cloud::Credential::new(self.runpod_key.trim().to_owned())?;
            }
        }
        self.hetzner
            .validate(self.settings.hetzner.as_ref(), &self.root.join("credentials/hetzner"))?;
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
            if mode == Authentication::ChatGpt {
                if agent != Agent::Codex {
                    return Err(Error::Invalid(
                        "ChatGPT plan authentication is supported only for Codex",
                    ));
                }
                if require_chatgpt_sign_in
                    && self.selected_agents().contains(&agent)
                    && self.chatgpt.is_none()
                    && self.chatgpt_status_error.is_some()
                {
                    return Err(Error::Invalid(
                        "Saved account status unavailable. Reopen settings after the other sign-in operation ends.",
                    ));
                }
                // Only Codex offers the mode; a selected Codex needs a renewable sign-in with plan access.
                if require_chatgpt_sign_in
                    && self.selected_agents().contains(&agent)
                    && !self
                        .chatgpt
                        .as_ref()
                        .is_some_and(super::chatgpt::Connection::can_use_plan)
                {
                    return Err(Error::Invalid(
                        "Sign in with ChatGPT and grant plan access before saving these settings",
                    ));
                }
                continue;
            }
            if !value.is_empty() || (self.selected_agents().contains(&agent) && mode == Authentication::ApiKey) {
                validate_input(value, saved)?;
            }
        }
        Ok(())
    }

    /// # Errors
    /// Saves private bindings atomically and creates a dedicated SSH key on first use.
    /// Caller must run this off the UI thread; it may invoke local ssh-keygen.
    pub fn save(self) -> Result<Settings> {
        self.save_with_chatgpt_gate(true)
    }

    fn save_with_chatgpt_gate(mut self, require_chatgpt_sign_in: bool) -> Result<Settings> {
        self.validate_with_chatgpt_gate(require_chatgpt_sign_in)?;
        let _chatgpt_session = if require_chatgpt_sign_in
            && self.openai_auth == Authentication::ChatGpt
            && self.selected_agents().contains(&Agent::Codex)
        {
            Some(
                super::chatgpt::lock_plan(
                    &self.root,
                    self.chatgpt.as_ref().map(|connection| connection.client_id.as_str()),
                )
                .map_err(|_| {
                    Error::Invalid("The saved ChatGPT sign-in changed or could not be verified; reload these settings")
                })?,
            )
        } else {
            None
        };
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
            if matches!(mode, Authentication::Subscription | Authentication::ChatGpt) {
                *binding = None;
            } else if !value.trim().is_empty() {
                *binding = Some(write.secret(name, value)?);
            }
        }
        self.settings.openai_auth = Some(self.openai_auth);
        self.settings.hetzner = self.hetzner.save(self.settings.hetzner.as_ref(), &mut write)?;
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
        super::github::remember(self.settings.github.is_some());
        Ok(self.settings)
    }
}

/// The one compute account a first cloud needs.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Provider {
    RunPod,
    Hetzner,
}

/// Saves `key` for `provider` with the machine's other settings, making an SSH identity on
/// first use, so a first cloud can be started without visiting the settings form.
/// Run off the UI thread: it may invoke `ssh-keygen`.
/// # Errors
/// As [`Draft::save`].
pub fn save_provider_key(root: &Path, provider: Provider, key: &str) -> Result<Settings> {
    let mut draft = Draft::load(root)?;
    match provider {
        Provider::RunPod => draft.runpod_key = Zeroizing::new(key.trim().to_owned()),
        Provider::Hetzner => {
            draft.hetzner.enabled = true;
            draft.hetzner.token = Zeroizing::new(key.trim().to_owned());
        }
    }
    // A provider key is its own concern; it saves without the agent-credential gate.
    draft.save_with_chatgpt_gate(false)
}

/// A settings file this machine wrote: its bytes before and after the write.
pub struct Committed {
    before: Option<Vec<u8>>,
    after: Vec<u8>,
}

/// Saves the GitHub App of Connect GitHub, changes its mode, or forgets it, and keeps
/// every other setting as it is. Unlike the form's save it needs no provider or agent
/// credential, since Connect GitHub can come first. Run off the UI thread, as
/// [`save_provider_key`]. An open form adopts the result with [`Draft::adopt_github`].
/// `shown` is the app ID the caller showed, `None` for no app: settings that name another
/// app, or none, were saved by another Horizon window since, and are left as they are, so
/// a stale window never replaces or disconnects an app it did not show. `cancel` is checked
/// under the settings lock, right before the commit, so a cancelled Connect flow saves
/// nothing.
/// # Errors
/// Malformed saved settings, settings that changed while this save ran or no longer name
/// `shown`, or a cancellation.
pub fn save_github(
    root: &Path,
    shown: Option<u64>,
    github: Option<super::github::Settings>,
    cancel: &super::Cancellation,
) -> Result<Committed> {
    let mut write = storage::Transaction::new(root)?;
    let before = match std::fs::read(root.join("settings.json")) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut settings = before.as_ref().map_or_else(
        || Ok(defaults(root)),
        |bytes| serde_json::from_slice::<Settings>(bytes).map_err(|_| Error::Json),
    )?;
    if settings.github.as_ref().map(|github| github.app_id) != shown {
        return Err(Error::Invalid(
            "GitHub was changed in another Horizon window. Close and reopen the settings to see it.",
        ));
    }
    settings.github = github;
    cancel.check()?;
    // Committed only over the bytes read above, so a concurrent writer is never overwritten.
    write.commit(&settings, before.as_deref())?;
    super::github::remember(settings.github.is_some());
    // The bytes this save committed, never a later read of a file another writer may change.
    let after = storage::encode(&settings)?;
    Ok(Committed { before, after })
}

impl Draft {
    /// Takes the `ChatGPT` sign-in that finished while the form was open, so its card and
    /// the Save validation see the stored connection without reopening the files.
    pub fn adopt_chatgpt(&mut self, connection: Option<super::chatgpt::Connection>) {
        self.chatgpt = connection;
        self.chatgpt_status_error = None;
    }

    /// Takes the GitHub App that [`save_github`] saved. When the form was opened from the
    /// file that the save replaced, the form now counts the saved file as its starting
    /// point, so its own Save does not see the save as a change made elsewhere.
    pub fn adopt_github(&mut self, committed: &Committed, github: Option<super::github::Settings>) {
        if self.original == committed.before {
            self.original = Some(committed.after.clone());
        }
        self.settings.github = github;
    }
}

/// The Codex authentication: the API key when bound, else the `ChatGPT` sign-in.
fn openai_authentication(binding: Option<&PathBuf>) -> Authentication {
    if binding.is_some() {
        Authentication::ApiKey
    } else {
        Authentication::ChatGpt
    }
}

/// The agent authentication implied by a saved key binding; `Subscription` stays the
/// login choice for the agents that offer it.
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
        openai_auth: None,
        anthropic_workspace_id: None,
        git_credentials: Vec::new(),
        browserstack_credentials: Vec::new(),
        hetzner: None,
        github: None,
        placement: None,
    }
}
