//! Who may rent cloud workers without a person. A machine without this section
//! rents none for an agent, so the machine owner opts in once, in this file.
use super::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The machine owner's limits for a cloud an agent rents without a person. Each limit
/// is a refusal, never a warning: an agent that passes every one may rent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentLifecycle {
    /// Provider IDs an agent may rent on, such as `runpod`. Empty rents nothing.
    pub providers: Vec<String>,
    /// The dearest hourly rate an agent's cloud may cost, in the provider's own
    /// currency. Storage and other provider charges are not part of a rate.
    pub max_hourly: f64,
    /// Repository directories an agent may deploy, each an absolute host path. A
    /// repository that is not listed is refused, so the list is the allowlist.
    pub repositories: Vec<PathBuf>,
    /// Whether an agent's cloud may continue without GitHub access when the sign-in
    /// asks a person. Without it the operation fails instead of renting on unverified
    /// access.
    #[serde(default)]
    pub allow_github_sign_in_skip: bool,
}

impl AgentLifecycle {
    /// # Errors
    /// Refuses an unknown or uncreatable provider, a rate that is not a positive
    /// number, and a relative repository path.
    pub fn validate(&self) -> Result<()> {
        if self.providers.is_empty() {
            return Err(Error::Invalid(
                "The agent lifecycle policy needs at least one provider an agent may rent on",
            ));
        }
        for provider in &self.providers {
            if !horizon_cloud::provider::by_id(provider).is_some_and(|known| known.creatable) {
                return Err(Error::Invalid(
                    "An agent lifecycle provider must be one Horizon can create clouds on, such as runpod",
                ));
            }
        }
        let mut providers = std::collections::BTreeSet::new();
        if !self.providers.iter().all(|provider| providers.insert(provider)) {
            return Err(Error::Invalid(
                "Name each agent lifecycle provider once, so its limit is unambiguous",
            ));
        }
        if !self.max_hourly.is_finite() || self.max_hourly <= 0.0 {
            return Err(Error::Invalid(
                "The agent lifecycle hourly limit must be a positive number",
            ));
        }
        if self.repositories.is_empty() {
            return Err(Error::Invalid(
                "The agent lifecycle policy needs at least one repository an agent may deploy",
            ));
        }
        if self.repositories.iter().any(|path| !path.is_absolute()) {
            return Err(Error::Invalid(
                "Agent lifecycle repositories must use absolute machine-local paths",
            ));
        }
        Ok(())
    }

    /// Whether an agent may rent a `provider` cloud of `repository` at `hourly`. A rate
    /// this machine could not quote is a refusal, because the limit cannot be checked.
    ///
    /// # Errors
    /// The repository is not allowed, the provider is not allowed, the rate is above the
    /// limit, or no rate was known.
    pub fn admits(&self, repository: &Path, provider: &str, hourly: Option<f64>) -> std::result::Result<(), Refusal> {
        if !self
            .repositories
            .iter()
            .any(|allowed| allowed == repository || allowed.canonicalize().is_ok_and(|path| path == repository))
        {
            return Err(Refusal::Repository);
        }
        if !self.providers.iter().any(|allowed| allowed == provider) {
            return Err(Refusal::Provider);
        }
        let Some(hourly) = hourly else {
            return Err(Refusal::Unpriced);
        };
        if hourly > self.max_hourly {
            return Err(Refusal::Above);
        }
        Ok(())
    }
}

/// Why an agent may not rent a cloud on this machine. Each is a refusal of the whole
/// operation, before anything is recorded or rented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The repository is not in the policy's list.
    Repository,
    /// The provider is not in the policy's list.
    Provider,
    /// This machine could not quote a rate, so the limit cannot be checked.
    Unpriced,
    /// The quoted rate is above `max_hourly`.
    Above,
}

impl Refusal {
    /// The sentence the caller of an agent's create reports.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Repository => "this repository is not one an agent may deploy",
            Self::Provider => "an agent may not rent on this provider",
            Self::Unpriced => "this machine could not quote an hourly rate, so no agent may rent",
            Self::Above => "the quoted hourly rate is above the agent limit",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> AgentLifecycle {
        let root = Path::new("/home/person/repositories");
        AgentLifecycle {
            providers: vec!["runpod".into()],
            max_hourly: 0.5,
            repositories: vec![root.join("shop")],
            allow_github_sign_in_skip: false,
        }
    }

    #[test]
    fn an_agent_rents_only_a_listed_repository_provider_and_rate() {
        let policy = policy();
        let repository = Path::new("/home/person/repositories/shop");
        assert!(policy.admits(repository, "runpod", Some(0.44)).is_ok());
        assert!(policy.admits(repository, "runpod", Some(0.5)).is_ok());
        for (repository, provider, hourly, refusal) in [
            (
                Path::new("/home/person/repositories/other"),
                "runpod",
                Some(0.44),
                Refusal::Repository,
            ),
            (
                Path::new("/home/person/repositories/shop/nested"),
                "runpod",
                Some(0.44),
                Refusal::Repository,
            ),
            (repository, "hetzner", Some(0.44), Refusal::Provider),
            (repository, "runpod", Some(0.51), Refusal::Above),
            (repository, "runpod", None, Refusal::Unpriced),
        ] {
            assert_eq!(policy.admits(repository, provider, hourly), Err(refusal));
        }
    }

    #[test]
    fn a_policy_without_a_usable_limit_is_refused_when_the_settings_load() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        let repository = root.path().join("repository");
        let base = serde_json::json!({
            "runpod_key_file": root.path().join("key"),
            "ssh_identity_file": root.path().join("identity"),
            "docker_config": root.path().join("docker"),
            "registry_pull_auth_id": null,
            "cpu_flavors": ["cpu3c"], "gpu_types": [], "data_centers": ["US-MO-2"],
        });
        std::fs::write(&path, base.to_string()).unwrap();
        assert!(
            load(&path).agent_lifecycle.is_none(),
            "a machine without the section rents nothing for an agent"
        );
        let valid = serde_json::json!({
            "providers": ["runpod"], "max_hourly": 0.5,
            "repositories": [repository], "allow_github_sign_in_skip": true,
        });
        let mut with_section = base.clone();
        with_section["agent_lifecycle"] = valid.clone();
        std::fs::write(&path, with_section.to_string()).unwrap();
        let loaded = load(&path).agent_lifecycle.unwrap();
        assert_eq!(loaded.repositories, [repository]);
        assert!(loaded.allow_github_sign_in_skip);
        for (field, value) in [
            ("providers", serde_json::json!([])),
            ("providers", serde_json::json!(["cardshop"])),
            ("providers", serde_json::json!(["runpod", "runpod"])),
            ("max_hourly", serde_json::json!(0)),
            ("max_hourly", serde_json::json!(-1.5)),
            ("repositories", serde_json::json!([])),
            ("repositories", serde_json::json!(["relative/repository"])),
            ("unknown", serde_json::json!(true)),
        ] {
            let mut invalid = with_section.clone();
            invalid["agent_lifecycle"][field] = value;
            std::fs::write(&path, invalid.to_string()).unwrap();
            assert!(super::super::Settings::load(&path).is_err(), "{field}");
        }
        // A policy the owner set without the optional skip stays refused for the skip.
        let mut without_skip = with_section;
        without_skip["agent_lifecycle"]
            .as_object_mut()
            .unwrap()
            .remove("allow_github_sign_in_skip");
        std::fs::write(&path, without_skip.to_string()).unwrap();
        assert!(!load(&path).agent_lifecycle.unwrap().allow_github_sign_in_skip);
    }

    /// The settings section as the machine file encodes it, with the defaults of an
    /// older file that has no section.
    fn load(path: &Path) -> super::super::Settings {
        super::super::Settings::load(path).unwrap()
    }
}
