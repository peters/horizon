use std::io::Write;

use super::SessionStore;
use crate::Result;

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

    /// Persist recovery information privately before an application can exit.
    ///
    /// # Errors
    /// Returns an error if the notice cannot be atomically written.
    pub fn save_session_deletion_notice(&self, notice: &str) -> Result<()> {
        let parent = self.deletion_notice_dir();
        std::fs::create_dir_all(&parent)?;
        let path = parent.join(format!("{}.txt", uuid::Uuid::new_v4()));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(notice.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path).map_err(|error| error.error)?;
        Ok(())
    }

    /// Acknowledge exactly the notice the person reviewed.
    ///
    /// # Errors
    /// Returns an error if another notice replaced it or removal fails.
    pub fn acknowledge_session_deletion_notice(&self, expected: &str) -> Result<()> {
        let notices = self.deletion_notices()?;
        if notices.is_empty() {
            return Ok(());
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
        // Records are immutable with unique names. New concurrent notices are
        // outside this snapshot and cannot be removed by this acknowledgement.
        for (path, _) in notices {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_notice_survives_reopen_until_matching_acknowledgement() {
        let root = tempfile::tempdir().expect("home");
        let store = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        store
            .save_session_deletion_notice("Retained bundle: /synthetic/recovery")
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
            store.save_session_deletion_notice("private").expect("save");
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
    fn independent_instances_preserve_each_others_unacknowledged_notices() {
        let root = tempfile::tempdir().expect("home");
        let first = SessionStore::new(
            crate::HorizonHome::from_root(root.path().into()),
            root.path().join("config.yaml"),
        );
        let second = first.clone();
        first
            .save_session_deletion_notice("First retained bundle")
            .expect("first");
        let old = first.saved_session_deletion_notice().expect("old").expect("notice");
        second
            .save_session_deletion_notice("Second retained bundle")
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
}
