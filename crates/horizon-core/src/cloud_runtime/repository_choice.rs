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
/// folder by its full path (`~` is the home folder; a relative folder that exists counts),
/// a link without a trailing slash or `.git`. `None` for an empty field.
#[must_use]
pub fn key(repository: &str) -> Option<String> {
    let trimmed = repository.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let path = crate::dir_search::expand_tilde(trimmed);
    // A folder keeps its whole name, also one that ends in `.git`. A relative folder that
    // exists is kept by its full path too, so `repo` and `./repo` are one repository.
    if path.is_absolute() || path.is_dir() {
        let full = path.canonicalize().unwrap_or(path);
        return Some(full.to_string_lossy().into_owned());
    }
    let link = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    (!link.is_empty()).then(|| link.to_owned())
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
    /// under `root`. The file is written and renamed into place here, so a save that cannot
    /// be written fails now. On Unix, making it durable can take seconds on a busy disk, so
    /// that runs on a worker thread; a crash before then can leave the file unreadable on a
    /// file system that does not write data before a rename. An unreadable file keeps
    /// nothing ([`Self::load`]), so every repository then asks again: the choices are a
    /// convenience, not worth a stalled frame. Windows cannot replace a file that another
    /// handle holds open, so there the file is made durable before the rename.
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
        #[cfg(not(unix))]
        file.as_file().sync_all()?;
        file.persist(root.join(FILE)).map_err(|error| error.error)?;
        self.0 = next;
        #[cfg(unix)]
        {
            let root = root.to_path_buf();
            std::thread::spawn(move || sync_in_background(&root));
        }
        Ok(())
    }
}

/// A test holds the sync of the choices under a folder until it sends, and learns when
/// the sync ran.
/// What a test holds a folder's sync with: the gate it opens, and where the sync says it ran.
#[cfg(all(test, unix))]
type SyncGate = (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>);

#[cfg(all(test, unix))]
static SYNC_GATES: std::sync::Mutex<BTreeMap<std::path::PathBuf, SyncGate>> = std::sync::Mutex::new(BTreeMap::new());

/// Makes the saved choices under `root`, and their name in the folder, durable.
#[cfg(unix)]
fn sync_in_background(root: &Path) {
    #[cfg(test)]
    let gate = SYNC_GATES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(root);
    #[cfg(test)]
    if let Some((opened, _)) = &gate {
        // Bounded, so a sync that runs on the caller fails the test instead of hanging it.
        opened.recv_timeout(std::time::Duration::from_secs(5)).ok();
    }
    let synced = std::fs::File::open(root.join(FILE))
        .and_then(|file| file.sync_all())
        .and_then(|()| std::fs::File::open(root)?.sync_all());
    if let Err(error) = synced {
        tracing::warn!(root = %root.display(), %error, "the repository choices are saved but not yet durable");
    }
    #[cfg(test)]
    if let Some((_, ran)) = gate {
        ran.send(()).ok();
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
        let folder = repository.path().join("project");
        let checkout = format!("{}.git", folder.display());
        assert_ne!(key(&checkout), key(&folder.to_string_lossy()), "a folder keeps .git");
        // A relative folder that exists is kept by its full path.
        let here = std::env::current_dir().unwrap();
        assert_eq!(
            key("."),
            Some(here.canonicalize().unwrap().to_string_lossy().into_owned())
        );
        let home = crate::dir_search::expand_tilde("~/code/app");
        assert_eq!(key("~/code/app"), key(&home.to_string_lossy()));
    }

    #[test]
    fn a_damaged_file_keeps_nothing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(FILE), b"{not json").unwrap();
        assert_eq!(Choices::load(root.path()), Choices::default());
    }

    #[test]
    #[cfg(unix)]
    fn a_kept_choice_is_saved_before_it_is_made_durable() {
        let root = tempfile::tempdir().unwrap();
        let (open, opened) = std::sync::mpsc::channel();
        let (ran, synced) = std::sync::mpsc::channel();
        SYNC_GATES
            .lock()
            .unwrap()
            .insert(root.path().to_path_buf(), (opened, ran));
        let mut choices = Choices::default();
        let link = "https://github.com/example/app";
        // The sync waits for the gate, so `set` returning shows that it does not wait.
        choices.set(root.path(), link, Some(WorkspacePlacement::Cloud)).unwrap();
        assert_eq!(
            Choices::load(root.path()).get(link),
            Some(WorkspacePlacement::Cloud),
            "the file is in place before it is durable"
        );
        assert!(synced.try_recv().is_err(), "the sync has not run yet");
        open.send(()).unwrap();
        synced.recv().unwrap();
    }
}
