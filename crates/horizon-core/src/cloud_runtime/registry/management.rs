use super::{Cancellation, Error, Journal, Prepared, Result, Settings, State, find};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Validation {
    pub image: String,
    pub scope: String,
    pub expires_at: Option<String>,
    pub checked_at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Verify { image: String },
    VerifyPull { image: String },
    Status { repository: String, generation: String },
    PullStatus { repository: String, generation: String },
    Reconcile { repository: String, generation: String },
    Revoke { repository: String, generation: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Provider,
    Host,
}

impl Action {
    #[must_use]
    pub fn scope(&self) -> Scope {
        if matches!(self, Self::VerifyPull { .. } | Self::PullStatus { .. }) {
            Scope::Host
        } else {
            Scope::Provider
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub scope: Scope,
    pub repository: String,
    pub generation: String,
    pub state: State,
    pub validation: Option<Validation>,
    pub configured_pull_expiry: Option<String>,
}

/// Shared UI/CLI entry point. Never publishes images or allocates compute.
/// # Errors
/// Returns redacted validation errors and leaves uncertain provider operations fenced.
pub fn manage(settings: &Settings, action: &Action, cancel: &Cancellation) -> Result<Status> {
    manage_with(settings, action, cancel, Prepared::verify_image)
}

fn manage_with(
    settings: &Settings,
    action: &Action,
    cancel: &Cancellation,
    verify: impl FnOnce(&mut Prepared, &str, &Cancellation) -> Result<()>,
) -> Result<Status> {
    cancel.check()?;
    let scope = action.scope();
    if let Action::Verify { image } | Action::VerifyPull { image } = action {
        let provider = if scope == Scope::Host {
            horizon_cloud::provider::Kind::DockerHost
        } else {
            horizon_cloud::provider::Kind::RunPod
        };
        let mut prepared = Prepared::for_image_for_provider(settings, image, None, false, provider)?
            .ok_or(Error::Invalid("Image has no explicit registry binding"))?;
        verify(&mut prepared, image, cancel)?;
        if scope == Scope::Provider {
            prepared.ensure_provider(&horizon_cloud::runpod::RunPod::new(settings.credential()?), cancel)?;
        }
        return Ok(Status {
            scope,
            repository: prepared.binding.repository.clone(),
            generation: prepared.binding.generation.clone(),
            state: prepared.journal.state().clone(),
            validation: prepared.journal.validation(),
            configured_pull_expiry: prepared.binding.pull.expires_at.clone(),
        });
    }
    let (repository, generation) = match action {
        Action::Status { repository, generation } | Action::PullStatus { repository, generation } => {
            (repository, generation)
        }
        Action::Reconcile { repository, generation } => {
            super::reconcile(settings, repository, generation, cancel)?;
            (repository, generation)
        }
        Action::Revoke { repository, generation } => {
            super::revoke(settings, repository, generation, cancel)?;
            (repository, generation)
        }
        Action::Verify { .. } | Action::VerifyPull { .. } => return Err(Error::Invalid("Invalid registry action")),
    };
    let (config, binding) = find(settings, repository, generation)?;
    let journal = match scope {
        Scope::Host => Journal::open_host_generation(config, binding, generation)?,
        Scope::Provider => Journal::open_generation(config, binding, generation, settings)?,
    };
    Ok(Status {
        scope,
        repository: repository.clone(),
        generation: generation.clone(),
        state: journal.state().clone(),
        validation: journal.validation(),
        configured_pull_expiry: (generation == &binding.generation)
            .then(|| binding.pull.expires_at.clone())
            .flatten(),
    })
}

#[cfg(test)]
mod tests;
