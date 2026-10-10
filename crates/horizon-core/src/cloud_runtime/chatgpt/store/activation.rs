//! Recoverable sign-in publication: select first, then publish credentials atomically.
use super::{
    ACTIVE_FILE, Error, Path, Record, Result, Zeroizing, directory, encode, file, fs, private_file, read_private,
};
use serde::{Deserialize, Serialize};

const JOURNAL: &str = "pending-sign-in";

/// No credentials enter the journal. The digest identifies the atomic record commit.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    version: u32,
    client_id: String,
    previous_active: Option<String>,
    digest: Vec<u8>,
}

pub(in super::super) fn activate(root: &Path, record: &Record) -> Result<()> {
    let bytes = prepare(root, record)?;
    publish(root, record, &bytes)
}

fn publish(root: &Path, record: &Record, bytes: &[u8]) -> Result<()> {
    let publish = (|| {
        super::set_active(root, &record.client_id)?;
        private_file(&file(root, &record.client_id), bytes)
    })();
    // A write can fail after rename but before directory sync. Recovery verifies the
    // committed bytes and completes durability, or restores the previous selection.
    let committed = recover(root)?;
    match publish {
        Ok(()) | Err(_) if committed => Ok(()),
        Err(error) => Err(error),
        Ok(()) => Err(Error::Invalid("the sign-in storage commit could not be verified")),
    }
}

fn prepare(root: &Path, record: &Record) -> Result<Zeroizing<Vec<u8>>> {
    if !super::valid_client_id(&record.client_id) {
        return Err(Error::Invalid("the client ID is not safe in a file name"));
    }
    let path = directory(root).join(JOURNAL);
    if read_private(&path)?.is_some() {
        return Err(Error::Invalid("another sign-in publication needs recovery"));
    }
    let bytes = encode(record)?;
    let pending = Pending {
        version: 1,
        client_id: record.client_id.clone(),
        previous_active: super::active_client_id(root)?,
        digest: ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref().to_vec(),
    };
    let journal = super::serialized(|writer| serde_json::to_writer(writer, &pending))?;
    private_file(&path, &journal)?;
    Ok(bytes)
}

/// Called while the common session lock is held. True means the intended record won.
pub(super) fn recover(root: &Path) -> Result<bool> {
    let path = directory(root).join(JOURNAL);
    let Some(bytes) = read_private(&path)? else {
        return Ok(false);
    };
    let pending: Pending = serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?;
    if pending.version != 1
        || !super::valid_client_id(&pending.client_id)
        || pending
            .previous_active
            .as_deref()
            .is_some_and(|id| !super::valid_client_id(id))
        || pending.digest.len() != ring::digest::SHA256_OUTPUT_LEN
    {
        return Err(Error::Malformed);
    }
    let committed = read_private(&file(root, &pending.client_id))?
        .is_some_and(|record| ring::digest::digest(&ring::digest::SHA256, &record).as_ref() == pending.digest);
    let selected = if committed {
        Some(&pending.client_id)
    } else {
        pending.previous_active.as_ref()
    };
    match selected {
        Some(client) if super::active_client_id(root)?.as_ref() != Some(client) => super::set_active(root, client)?,
        None => {
            // Verify before removing an existing pointer, including its privacy.
            let _ = super::active_client_id(root)?;
            match fs::remove_file(directory(root).join(ACTIVE_FILE)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Some(_) => {}
    }
    sync_directory(root)?;
    fs::remove_file(path)?;
    sync_directory(root)?;
    Ok(committed)
}

fn sync_directory(root: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(directory(root))?.sync_all()?;
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(client: &str) -> Record {
        Record {
            email: Some("user@example.com".into()),
            issuer: "https://auth.openai.com".into(),
            subject: "synthetic-account".into(),
            client_id: client.into(),
            ext_agent_host_id: "synthetic-host".into(),
            id_token: Zeroizing::new("synthetic-id".into()),
            access_token: Some(Zeroizing::new("synthetic-access".into())),
            refresh_token: Some(Zeroizing::new("synthetic-refresh".into())),
            token_type: Some("Bearer".into()),
            expires_in: Some(3600),
            earliest_refresh_at: None,
            scopes: vec!["chatgpt.tokens.use.direct".into()],
            usage_confirmed: false,
            saved_at_unix: 1,
        }
    }

    #[test]
    fn sign_in_publishes_a_complete_pair_and_removes_its_journal() {
        let root = tempfile::tempdir().unwrap();
        let guard = super::super::session_lock(root.path()).unwrap();
        activate(root.path(), &record("client-a")).unwrap();
        drop(guard);
        assert!(
            super::super::super::status(root.path())
                .unwrap()
                .unwrap()
                .can_use_plan()
        );
        assert!(!directory(root.path()).join(JOURNAL).exists());
    }

    #[test]
    fn interrupted_publication_recovers_each_durable_commit_boundary() {
        for phase in ["journal", "selection", "record"] {
            for previous in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let guard = super::super::session_lock(root.path()).unwrap();
                if previous {
                    super::super::save(root.path(), &record("client-old")).unwrap();
                    super::super::set_active(root.path(), "client-old").unwrap();
                }
                let new = record("client-new");
                let bytes = prepare(root.path(), &new).unwrap();
                if phase != "journal" {
                    super::super::set_active(root.path(), "client-new").unwrap();
                }
                if phase == "record" {
                    private_file(&file(root.path(), "client-new"), &bytes).unwrap();
                }
                // Readers must fail closed until the interrupted writer releases its lock.
                assert!(super::super::super::status(root.path()).is_err());
                drop(guard);
                let status = super::super::super::status(root.path()).unwrap();
                let expected = if phase == "record" {
                    Some("client-new")
                } else if previous {
                    Some("client-old")
                } else {
                    None
                };
                assert_eq!(status.as_ref().map(|value| value.client_id.as_str()), expected);
                assert_eq!(
                    super::super::active_client_id(root.path()).unwrap().as_deref(),
                    expected
                );
                assert!(!directory(root.path()).join(JOURNAL).exists());
                assert_eq!(file(root.path(), "client-new").exists(), phase == "record");
            }
        }
    }

    #[test]
    fn activation_failure_does_not_publish_the_new_credentials() {
        let root = tempfile::tempdir().unwrap();
        let guard = super::super::session_lock(root.path()).unwrap();
        let new = record("client-new");
        let bytes = prepare(root.path(), &new).unwrap();
        // A directory at the pointer destination makes its atomic write fail.
        fs::create_dir(directory(root.path()).join(ACTIVE_FILE)).unwrap();
        assert!(publish(root.path(), &new, &bytes).is_err());
        assert!(!file(root.path(), "client-new").exists());
        drop(guard);
        fs::remove_dir(directory(root.path()).join(ACTIVE_FILE)).unwrap();
        assert!(super::super::super::status(root.path()).unwrap().is_none());
    }
}
