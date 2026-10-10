//! Read registrations and their selected account under one session guard.
use super::{
    Connection, Error, Path, Record, Result, SessionLock, directory, directory_exists, fs, read_record,
    verify_directory,
};

fn read_guard(root: &Path) -> Result<Option<SessionLock>> {
    let path = directory(root);
    if !directory_exists(&path)? {
        return Ok(None);
    }
    // Readers must refuse an exposed directory before a writer could repair it.
    verify_directory(&path)?;
    super::session_lock(root).map(Some)
}

pub(in super::super) fn registration(lock: &SessionLock, client_id: &str) -> Result<Option<Record>> {
    if !super::valid_client_id(client_id) {
        return Err(Error::Invalid("the client ID is not safe in a file name"));
    }
    read_record(&super::file(lock.root(), client_id))
}

pub(in super::super) fn connections(root: &Path) -> Result<Vec<Connection>> {
    let Some(lock) = read_guard(root)? else {
        return Ok(Vec::new());
    };
    Ok(read_records(&lock)?.into_iter().map(Connection::from).collect())
}

pub(in super::super) fn default_registration(root: &Path) -> Result<Option<Record>> {
    let Some(lock) = read_guard(root)? else {
        return Ok(None);
    };
    default_registration_locked(&lock)
}

pub(in super::super) fn default_registration_locked(lock: &SessionLock) -> Result<Option<Record>> {
    selected_registration(lock, super::active_client_id)
}

fn selected_registration(
    lock: &SessionLock,
    read_pointer: impl FnOnce(&Path) -> Result<Option<String>>,
) -> Result<Option<Record>> {
    let mut found = read_records(lock)?;
    let active = read_pointer(lock.root())?;
    let index = active
        .as_ref()
        .and_then(|id| found.iter().position(|record| &record.client_id == id));
    Ok(index
        .map(|index| found.remove(index))
        .or_else(|| found.into_iter().next()))
}

fn read_records(lock: &SessionLock) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory(lock.root()))? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        if !name.to_ascii_lowercase().ends_with(".json") {
            continue;
        }
        if let Some(record) = read_record(&entry.path())? {
            if name != format!("{}.json", record.client_id) {
                return Err(Error::Malformed);
            }
            records.push(record);
        }
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.saved_at_unix));
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_writer_cannot_change_selection_between_the_records_and_pointer_read() {
        let root = tempfile::tempdir().unwrap();
        super::super::save(
            root.path(),
            &super::super::tests::test_record("client-old", "account-old"),
        )
        .unwrap();
        super::super::set_active(root.path(), "client-old").unwrap();
        let guard = read_guard(root.path()).unwrap().unwrap();
        let account = selected_registration(&guard, |root| {
            std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        assert!(matches!(super::super::session_lock(root), Err(Error::Invalid(_))));
                        assert!(default_registration(root).is_err());
                        assert!(connections(root).is_err());
                    })
                    .join()
                    .unwrap();
            });
            super::super::active_client_id(root)
        })
        .unwrap()
        .unwrap();
        assert_eq!(account.client_id, "client-old");
        assert_eq!(account.subject, "account-old");
        drop(guard);
        let writer = super::super::session_lock(root.path()).unwrap();
        super::super::activate(
            root.path(),
            &super::super::tests::test_record("client-new", "account-new"),
        )
        .unwrap();
        assert_eq!(
            default_registration_locked(&writer).unwrap().unwrap().client_id,
            "client-new"
        );
        drop(writer);
        assert_eq!(
            default_registration(root.path()).unwrap().unwrap().client_id,
            "client-new"
        );
    }

    #[test]
    fn reading_an_absent_store_does_not_create_credentials_or_lock_files() {
        let root = tempfile::tempdir().unwrap();
        assert!(default_registration(root.path()).unwrap().is_none());
        assert!(connections(root.path()).unwrap().is_empty());
        assert!(!directory(root.path()).exists());
    }
}
