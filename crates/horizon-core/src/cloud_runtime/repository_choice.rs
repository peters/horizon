//! Where the person keeps each repository: in the cloud or on This PC, chosen in New cloud.
//! The choice lives only on this computer, in `repository-choices.json` beside the cloud
//! settings. Repository YAML never sets it; its `placement` is only the default.
use super::{Error, Result};
use crate::cloud_panel::WorkspacePlacement;
use std::{collections::BTreeMap, io::Write as _, path::Path};

const FILE: &str = "repository-choices.json";
/// More repositories than a person keeps; a larger file is not this one.
const MAX_ENTRIES: usize = 1_000;

/// The kept choice of each repository, by [`key`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices(BTreeMap<String, WorkspacePlacement>);

/// The name under which `repository`, as the New cloud field holds it, is kept: a local
/// folder by its full path (`~` is the home folder), a link without a trailing slash or
/// `.git`. `None` for an empty field.
#[must_use]
pub fn key(repository: &str) -> Option<String> {
    let trimmed = repository.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    if trimmed.is_empty() {
        return None;
    }
    let path = crate::dir_search::expand_tilde(trimmed);
    if path.is_absolute() {
        let full = path.canonicalize().unwrap_or(path);
        return Some(full.to_string_lossy().into_owned());
    }
    Some(trimmed.to_owned())
}

impl Choices {
    /// The kept choices under the cloud settings folder `root`. A missing or unreadable file
    /// keeps nothing: each repository then asks again.
    #[must_use]
    pub fn load(root: &Path) -> Self {
        std::fs::read(root.join(FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeMap<String, WorkspacePlacement>>(&bytes).ok())
            .filter(|choices| choices.len() <= MAX_ENTRIES)
            .map(Self)
            .unwrap_or_default()
    }

    /// The kept choice of `repository`.
    #[must_use]
    pub fn get(&self, repository: &str) -> Option<WorkspacePlacement> {
        self.0.get(&key(repository)?).copied()
    }

    /// Keeps `choice` for `repository`, or forgets it with `None`, and saves all choices
    /// under `root`.
    ///
    /// # Errors
    /// The field is empty, too many repositories are kept, or the file cannot be written.
    pub fn set(&mut self, root: &Path, repository: &str, choice: Option<WorkspacePlacement>) -> Result<()> {
        let key = key(repository).ok_or(Error::Invalid("No repository to keep a choice for"))?;
        let mut next = self.0.clone();
        match choice {
            Some(choice) => next.insert(key, choice),
            None => next.remove(&key),
        };
        if next.len() > MAX_ENTRIES {
            return Err(Error::Invalid("Too many repositories keep a choice"));
        }
        let bytes = serde_json::to_vec_pretty(&next).map_err(|_| Error::Json)?;
        std::fs::create_dir_all(root)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(root.join(FILE)).map_err(|error| error.error)?;
        self.0 = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_choice_survives_a_reload_and_can_be_forgotten() {
        let root = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let path = repository.path().to_string_lossy().into_owned();
        let mut choices = Choices::load(root.path());
        assert_eq!(choices.get(&path), None);

        choices
            .set(root.path(), &format!("{path}/"), Some(WorkspacePlacement::Local))
            .unwrap();
        choices
            .set(
                root.path(),
                "https://github.com/example/app.git",
                Some(WorkspacePlacement::Cloud),
            )
            .unwrap();
        let reloaded = Choices::load(root.path());
        assert_eq!(reloaded.get(&path), Some(WorkspacePlacement::Local));
        assert_eq!(
            reloaded.get("https://github.com/example/app"),
            Some(WorkspacePlacement::Cloud)
        );

        choices.set(root.path(), &path, None).unwrap();
        assert_eq!(Choices::load(root.path()).get(&path), None);
        assert!(choices.set(root.path(), "  ", Some(WorkspacePlacement::Local)).is_err());
        let home = crate::dir_search::expand_tilde("~/code/app");
        assert_eq!(key("~/code/app"), key(&home.to_string_lossy()));
    }

    #[test]
    fn a_damaged_file_keeps_nothing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(FILE), b"{not json").unwrap();
        assert_eq!(Choices::load(root.path()), Choices::default());
    }
}
