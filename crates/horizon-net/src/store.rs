use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{AgentConfig, Error, Result, Topology, model::parse_key};

const MAX_PRIVATE_BYTES: usize = 1_048_576;

#[derive(Clone)]
pub(crate) struct Store {
    _ownership: Arc<std::fs::File>,
    directory: PathBuf,
    authority_key: String,
    node_key: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    authority_key: String,
    node_key: String,
    topology: Topology,
}

impl Store {
    pub(crate) fn enrolled_key(&self) -> Result<iroh::EndpointId> {
        parse_key(&self.node_key)
    }
    pub(crate) fn open(directory: PathBuf, config: &mut AgentConfig) -> Result<Self> {
        if let Ok(metadata) = fs::symlink_metadata(&directory) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(Error::InvalidConfiguration(
                    "state directory must be a real directory".into(),
                ));
            }
        } else {
            fs::create_dir_all(&directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            }
        }
        validate_private(&directory, true)?;
        let lock_path = directory.join("agent.lock");
        if lock_path.exists() {
            validate_private(&lock_path, false)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let ownership = options.open(&lock_path)?;
        ownership
            .try_lock()
            .map_err(|_| Error::InvalidConfiguration("another agent owns the state directory".into()))?;
        let node_key = config
            .topology
            .nodes
            .get(&config.node)
            .ok_or(Error::Denied)?
            .key
            .clone();
        let store = Self {
            _ownership: Arc::new(ownership),
            directory,
            authority_key: config.authority_key.clone(),
            node_key,
        };
        let mut latest = config.topology.clone();
        for entry in fs::read_dir(&store.directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "json") {
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(Error::InvalidConfiguration(
                        "state snapshot must be a regular file".into(),
                    ));
                }
                let snapshot: Snapshot = serde_json::from_slice(&read_private(&path)?)?;
                snapshot.topology.validate()?;
                if parse_key(&snapshot.authority_key)? != parse_key(&store.authority_key)?
                    || parse_key(&snapshot.node_key)? != parse_key(&store.node_key)?
                    || snapshot.topology.network != config.topology.network
                {
                    return Err(Error::InvalidConfiguration(
                        "persisted authority or network does not match configuration".into(),
                    ));
                }
                if snapshot.topology.revision > latest.revision {
                    latest = snapshot.topology;
                } else if snapshot.topology.revision == latest.revision && snapshot.topology != latest {
                    return Err(Error::InvalidConfiguration(
                        "conflicting persisted topology revision".into(),
                    ));
                }
            }
        }
        config.topology = latest;
        store.persist(&config.topology)?;
        Ok(store)
    }

    pub(crate) fn persist(&self, topology: &Topology) -> Result<()> {
        let snapshot = Snapshot {
            authority_key: self.authority_key.clone(),
            node_key: self.node_key.clone(),
            topology: topology.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot)?;
        if bytes.len() > MAX_PRIVATE_BYTES {
            return Err(Error::MessageTooLarge);
        }
        let final_path = self.directory.join(format!("{:020}.json", topology.revision));
        if final_path.exists() {
            let previous: Snapshot = serde_json::from_slice(&read_private(&final_path)?)?;
            if previous.topology == *topology
                && previous.authority_key == self.authority_key
                && previous.node_key == self.node_key
            {
                return Ok(());
            }
            return Err(Error::StalePlan);
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::InvalidConfiguration("clock precedes Unix epoch".into()))?
            .as_nanos();
        let temporary = self.directory.join(format!("{}-{nonce}.tmp", std::process::id()));
        write_private(&temporary, &bytes)?;
        // Hard-link creation is atomic and fails if the immutable revision
        // already exists; unlike rename it cannot overwrite another writer.
        fs::hard_link(&temporary, &final_path)?;
        fs::remove_file(&temporary)?;
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}

pub(crate) fn read_private(path: &Path) -> Result<Vec<u8>> {
    validate_private(path, false)?;
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, (MAX_PRIVATE_BYTES + 1) as u64),
        &mut bytes,
    )?;
    if bytes.len() > MAX_PRIVATE_BYTES {
        return Err(Error::MessageTooLarge);
    }
    Ok(bytes)
}

fn validate_private(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || if directory {
            !metadata.is_dir()
        } else {
            !metadata.is_file()
        }
    {
        return Err(Error::InvalidConfiguration(
            "configuration/state must use regular private files".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let maximum = if directory { 0o700 } else { 0o600 };
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o7777 != maximum {
            return Err(Error::InvalidConfiguration(
                "configuration/state must be owned by this user with mode 0700/0600".into(),
            ));
        }
    }
    if !directory && metadata.len() > MAX_PRIVATE_BYTES as u64 {
        return Err(Error::MessageTooLarge);
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;

    fn config() -> AgentConfig {
        let mut topology = Topology::empty("store-island");
        let key = iroh::SecretKey::from_bytes(&[1; 32]).public().to_string();
        topology.nodes.insert("node".into(), Node { key: key.clone() });
        AgentConfig {
            node: "node".into(),
            secret_key: String::new(),
            authority_key: key,
            topology,
            relay_urls: vec!["https://relay.example.com".into()],
            relay_only: true,
        }
    }

    #[test]
    fn concurrent_agent_cannot_own_or_overwrite_state() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let mut initial = config();
        let store = Store::open(directory.clone(), &mut initial)?;
        let clone = store.clone();
        assert!(Store::open(directory.clone(), &mut config()).is_err());
        let mut conflicting = initial.topology.clone();
        conflicting.nodes.clear();
        assert!(matches!(store.persist(&conflicting), Err(Error::StalePlan)));
        drop(store);
        assert!(Store::open(directory.clone(), &mut config()).is_err());
        drop(clone);
        assert!(Store::open(directory, &mut config()).is_ok());
        Ok(())
    }

    #[test]
    fn oversized_snapshot_cannot_publish_or_advance_the_controller() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let mut initial = config();
        let store = Store::open(directory.clone(), &mut initial)?;
        let controller = crate::Controller::new(initial.topology.clone())?;
        let mut proposed = initial.topology.clone();
        proposed.revision = 1;
        for index in 0..7_000 {
            proposed.services.insert(
                format!("{index:04}-{}", "s".repeat(123)),
                crate::Service {
                    node: "node".into(),
                    port: 22,
                },
            );
        }
        proposed.validate()?;
        let plan = controller.plan(proposed)?;
        assert!(matches!(
            controller.apply_persisted(&plan, |topology| store.persist(topology)),
            Err(Error::MessageTooLarge)
        ));
        assert_eq!(controller.topology(), initial.topology);
        assert!(!directory.join("00000000000000000001.json").exists());
        assert_eq!(
            fs::read_dir(&directory)?.count(),
            2,
            "only initial revision and its ownership lock"
        );
        drop(store);
        let _restored = Store::open(directory, &mut initial)?;
        assert_eq!(initial.topology.revision, 0);
        Ok(())
    }

    #[test]
    fn committed_corrupt_state_fails_closed() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let store = Store::open(directory.clone(), &mut config())?;
        write_private(&directory.join("00000000000000000001.json"), b"invalid json")?;
        drop(store);
        assert!(Store::open(directory, &mut config()).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn permissive_files_and_oversized_config_are_rejected() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("config.json");
        fs::write(&path, b"{}")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        assert!(read_private(&path).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        assert_eq!(read_private(&path)?, b"{}");
        fs::write(&path, vec![0; 1_048_577])?;
        assert!(matches!(read_private(&path), Err(Error::MessageTooLarge)));
        Ok(())
    }
}
