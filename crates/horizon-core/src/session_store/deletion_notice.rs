use std::io::{Read, Write};

use super::SessionStore;
use crate::Result;

/// An immutable recovery receipt whose identity survives persistence retries.
#[derive(Clone, Debug)]
pub struct SessionDeletionNotice {
    id: uuid::Uuid,
    text: String,
}

impl SessionDeletionNotice {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            text: text.into(),
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl SessionStore {
    fn deletion_notice_dir(&self) -> std::path::PathBuf {
        self.home.root().join("runtime/session-deletion").join(&self.profile_id)
    }

    fn deletion_notices(&self) -> Result<Vec<(std::path::PathBuf, String)>> {
        let entries = match std::fs::read_dir(self.deletion_notice_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut notices = Vec::new();
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "txt") {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(notice) => notices.push((path, notice)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        notices.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(notices)
    }

    /// Read unresolved deletion recovery information for this profile.
    ///
    /// # Errors
    /// Returns an error if an existing notice cannot be read.
    pub fn saved_session_deletion_notice(&self) -> Result<Option<String>> {
        let notices = self.deletion_notices()?;
        Ok((!notices.is_empty()).then(|| {
            notices
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        }))
    }

    fn sync_deletion_notice_directories(&self) -> Result<()> {
        if !cfg!(unix) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Durable recovery notices require Unix directory synchronization",
            )
            .into());
        }
        let directory = self.deletion_notice_dir();
        for ancestor in directory.ancestors() {
            match std::fs::File::open(ancestor) {
                Ok(directory) => directory.sync_all()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if Some(ancestor) == self.home.root().parent() {
                break;
            }
        }
        Ok(())
    }

    /// Persist recovery information privately before an application can exit.
    /// Reuse the same notice on retry to synchronize any uncertain publication.
    ///
    /// # Errors
    /// Returns an error if the notice cannot be durably saved or its identity
    /// already belongs to different contents.
    pub fn save_session_deletion_notice(&self, notice: &SessionDeletionNotice) -> Result<()> {
        self.save_deletion_notice_with_sync(notice, || self.sync_deletion_notice_directories())
    }

    fn save_deletion_notice_with_sync(
        &self,
        notice: &SessionDeletionNotice,
        sync: impl Fn() -> Result<()>,
    ) -> Result<()> {
        sync()?;
        let parent = self.deletion_notice_dir();
        std::fs::create_dir_all(&parent)?;
        sync()?;
        let path = parent.join(format!("{}.txt", notice.id));
        match std::fs::File::open(&path) {
            Ok(mut file) => {
                let mut existing = String::new();
                file.read_to_string(&mut existing)?;
                if existing != notice.text {
                    return Err(crate::Error::State(
                        "Recovery receipt identity has different contents".into(),
                    ));
                }
                file.sync_all()?;
                return sync();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut file = tempfile::NamedTempFile::new_in(&parent)?;
        file.write_all(notice.text.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&path).map_err(|error| error.error)?;
        sync()
    }

    /// Acknowledge exactly the notice the person reviewed.
    ///
    /// # Errors
    /// Returns an error if another notice replaced it or removal fails.
    pub fn acknowledge_session_deletion_notice(&self, expected: &str) -> Result<()> {
        self.acknowledge_deletion_notice_with_sync(expected, || self.sync_deletion_notice_directories())
    }

    fn acknowledge_deletion_notice_with_sync(&self, expected: &str, sync: impl Fn() -> Result<()>) -> Result<()> {
        let notices = self.deletion_notices()?;
        if notices.is_empty() {
            return sync();
        }
        let current = notices
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if current != expected {
            return Err(crate::Error::State(
                "Deletion recovery information changed; reopen the notice".into(),
            ));
        }
        sync()?;
        // Records are immutable with unique names. New concurrent notices are
        // outside this snapshot and cannot be removed by this acknowledgement.
        for (path, _) in notices {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg_attr(
        windows,
        ignore = "Durable notices require Unix directory synchronization; unsupported Windows preservation is tested separately"
    )]
    fn recovery_notice_survives_reopen_until_matching_acknowledgement() {
        let root = tempfile::tempdir().expect("home");
        let store = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        store
            .save_session_deletion_notice(&SessionDeletionNotice::new("Retained bundle: /synthetic/recovery"))
            .expect("save");
        let reopened = SessionStore::new(store.home().clone(), store.config_path().into());
        assert_eq!(
            reopened.saved_session_deletion_notice().expect("read").as_deref(),
            Some("Retained bundle: /synthetic/recovery")
        );
        assert!(reopened.acknowledge_session_deletion_notice("older notice").is_err());
        reopened
            .acknowledge_session_deletion_notice("Retained bundle: /synthetic/recovery")
            .expect("acknowledge");
        assert!(store.saved_session_deletion_notice().expect("cleared").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            store
                .save_session_deletion_notice(&SessionDeletionNotice::new("private"))
                .expect("save");
            assert_eq!(
                std::fs::metadata(store.deletion_notices().expect("records")[0].0.clone())
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
    }
    #[test]
    #[cfg_attr(
        windows,
        ignore = "Durable notices require Unix directory synchronization; unsupported Windows preservation is tested separately"
    )]
    fn independent_instances_preserve_each_others_unacknowledged_notices() {
        let root = tempfile::tempdir().expect("home");
        let first = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        let second = first.clone();
        first
            .save_session_deletion_notice(&SessionDeletionNotice::new("First retained bundle"))
            .expect("first");
        let old = first.saved_session_deletion_notice().expect("old").expect("notice");
        second
            .save_session_deletion_notice(&SessionDeletionNotice::new("Second retained bundle"))
            .expect("second");
        assert!(first.acknowledge_session_deletion_notice(&old).is_err());
        let current = second.saved_session_deletion_notice().expect("read").expect("notices");
        assert!(current.contains("First retained bundle"));
        assert!(current.contains("Second retained bundle"));
        second
            .acknowledge_session_deletion_notice(&current)
            .expect("acknowledge both");
        assert!(first.saved_session_deletion_notice().expect("read").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn failed_publication_sync_retains_the_recovery_record() {
        let root = tempfile::tempdir().expect("home");
        let store = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        let calls = std::cell::Cell::new(0);
        let notice = SessionDeletionNotice::new("Retained recovery path");
        let result = store.save_deletion_notice_with_sync(&notice, || {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                return Err(std::io::Error::other("publication sync failed").into());
            }
            store.sync_deletion_notice_directories()
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 3);
        assert_eq!(
            store.saved_session_deletion_notice().expect("read").as_deref(),
            Some("Retained recovery path")
        );
        store
            .save_session_deletion_notice(&notice)
            .expect("retry published receipt");
        assert_eq!(store.deletion_notices().expect("records").len(), 1);
        assert_eq!(
            store.saved_session_deletion_notice().expect("read").as_deref(),
            Some("Retained recovery path")
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_acknowledgement_sync_is_retried_after_unlink() {
        let root = tempfile::tempdir().expect("home");
        let store = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        store
            .save_session_deletion_notice(&SessionDeletionNotice::new("Reviewed recovery path"))
            .expect("save");
        let calls = std::cell::Cell::new(0);
        let result = store.acknowledge_deletion_notice_with_sync("Reviewed recovery path", || {
            calls.set(calls.get() + 1);
            if calls.get() == 2 {
                return Err(std::io::Error::other("acknowledgement sync failed").into());
            }
            store.sync_deletion_notice_directories()
        });
        assert!(result.is_err());
        assert!(store.saved_session_deletion_notice().expect("read").is_none());
        store
            .acknowledge_deletion_notice_with_sync("Reviewed recovery path", || {
                calls.set(calls.get() + 1);
                store.sync_deletion_notice_directories()
            })
            .expect("retry synchronization");
        assert_eq!(calls.get(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn receipt_identity_is_immutable_and_distinct_equal_text_is_preserved() {
        let root = tempfile::tempdir().expect("home");
        let store = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        let notice = SessionDeletionNotice::new("Recovery path");
        store.save_session_deletion_notice(&notice).expect("first");
        let mut changed = notice.clone();
        changed.text = "Different recovery path".into();
        assert!(store.save_session_deletion_notice(&changed).is_err());
        assert_eq!(
            store.saved_session_deletion_notice().expect("read").as_deref(),
            Some("Recovery path")
        );
        store
            .save_session_deletion_notice(&SessionDeletionNotice::new("Recovery path"))
            .expect("independent record");
        assert_eq!(store.deletion_notices().expect("records").len(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn unsupported_notice_durability_creates_no_partial_record() {
        let root = tempfile::tempdir().expect("home");
        let store = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        assert!(
            store
                .save_session_deletion_notice(&SessionDeletionNotice::new("keep this recovery path"))
                .is_err()
        );
        assert!(!store.deletion_notice_dir().exists());
    }
}
