//! Saves only one local publishing preference, preserving other windows' settings.
use super::{Committed, Error, Path, Result, Settings, storage};
use crate::cloud_runtime::{
    Cancellation,
    publishing::{Override, Selection},
};

/// Saves a destination for one canonical checkout and named profile. `None` restores
/// the repository profile's default. `shown` guards against changing another window's choice.
/// The trusted cloud root must exist; its settings file can be absent.
/// Run on a worker thread. No repository file or credential is changed.
/// # Errors
/// A malformed scope, a destination without a publishing grant, stale choice or failed write.
pub fn save_image_destination(
    root: &Path,
    repository: &Path,
    profile: &str,
    shown: Option<&Override>,
    destination: Option<&Selection>,
    cancel: &Cancellation,
) -> Result<Committed> {
    cancel.check()?;
    let repository = repository.canonicalize()?;
    if !horizon_cloud::valid_id(profile) {
        return Err(Error::Invalid(
            "Choose a named repository profile before saving its image destination",
        ));
    }
    if root.canonicalize()?.starts_with(&repository) {
        return Err(Error::Invalid(
            "Image destination settings must stay outside the source repository",
        ));
    }
    let mut write = storage::Transaction::new(root)?;
    let before = match std::fs::read(root.join("settings.json")) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut settings: Settings = before.as_ref().map_or_else(
        || Ok(super::defaults(root)),
        |bytes| serde_json::from_slice(bytes).map_err(|_| Error::Json),
    )?;
    settings.validate()?;
    let matches = |choice: &&Override| choice.repository == repository && choice.profile == profile;
    let observed = settings.image_publishing.iter().find(matches);
    if observed != shown {
        return Err(Error::Invalid(
            "This profile's image destination changed in another window; reload before saving",
        ));
    }
    if let Some(destination) = destination {
        destination.check_binding(&settings)?;
        if !matches!(destination, Selection::Registry { .. } | Selection::Personal { .. }) {
            return Err(Error::Invalid(
                "Choose an existing registry binding or restore the repository default",
            ));
        }
    }
    let replacement = destination.map(|destination| Override {
        repository: repository.clone(),
        profile: profile.to_owned(),
        destination: destination.repository().unwrap_or_default().to_owned(),
        binding: match destination {
            Selection::Registry { destination } => match &destination.publisher {
                horizon_cloud::image_publishing::Publisher::Registry { binding } => Some(binding.clone()),
                horizon_cloud::image_publishing::Publisher::GitHub { .. } => None,
            },
            _ => None,
        },
        account: match destination {
            Selection::Personal { account, .. } => Some(account.clone()),
            _ => None,
        },
    });
    if let Some(choice) = &replacement {
        choice.validate(&settings)?;
    }
    settings
        .image_publishing
        .retain(|choice| choice.repository != repository || choice.profile != profile);
    settings.image_publishing.extend(replacement);
    cancel.check()?;
    write.commit(&settings, before.as_deref())?;
    let after = storage::encode(&settings)?;
    Ok(Committed { before, after })
}

#[cfg(test)]
mod tests;
