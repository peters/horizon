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
    Status { repository: String, generation: String },
    Reconcile { repository: String, generation: String },
    Revoke { repository: String, generation: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub repository: String,
    pub generation: String,
    pub state: State,
    pub validation: Option<Validation>,
}

/// Shared UI/CLI entry point. Never publishes images or allocates compute.
/// # Errors
/// Returns redacted validation errors and leaves uncertain provider operations fenced.
pub fn manage(settings: &Settings, action: &Action, cancel: &Cancellation) -> Result<Status> {
    cancel.check()?;
    if let Action::Verify { image } = action {
        let mut prepared = Prepared::for_image(settings, image, None, false)?
            .ok_or(Error::Invalid("Image has no explicit registry binding"))?;
        prepared.verify_image(image, cancel)?;
        prepared.ensure_provider(&horizon_cloud::runpod::RunPod::new(settings.credential()?), cancel)?;
        return Ok(Status {
            repository: prepared.binding.repository.clone(),
            generation: prepared.binding.generation.clone(),
            state: prepared.journal.state().clone(),
            validation: prepared.journal.validation(),
        });
    }
    let (repository, generation) = match action {
        Action::Status { repository, generation } => (repository, generation),
        Action::Reconcile { repository, generation } => {
            super::reconcile(settings, repository, generation, cancel)?;
            (repository, generation)
        }
        Action::Revoke { repository, generation } => {
            super::revoke(settings, repository, generation, cancel)?;
            (repository, generation)
        }
        Action::Verify { .. } => return Err(Error::Invalid("Invalid registry action")),
    };
    let (config, binding) = find(settings, repository, generation)?;
    let journal = Journal::open_generation(config, binding, generation, settings)?;
    Ok(Status {
        repository: repository.clone(),
        generation: generation.clone(),
        state: journal.state().clone(),
        validation: journal.validation(),
    })
}
