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
        require_directory_durability()?;
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
        let directory = fs::canonicalize(directory)?;
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
        let mut latest: Option<Topology> = None;
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
                match &latest {
                    Some(previous) if snapshot.topology.revision < previous.revision => {}
                    Some(previous) if snapshot.topology.revision == previous.revision => {
                        if snapshot.topology != *previous {
                            return Err(Error::InvalidConfiguration(
                                "conflicting persisted topology revision".into(),
                            ));
                        }
                    }
                    _ => latest = Some(snapshot.topology),
                }
            }
        }
        if let Some(latest) = latest {
            config.topology = latest;
        }
        store.persist(&config.topology)?;
        Ok(store)
    }

    pub(crate) fn persist(&self, topology: &Topology) -> Result<()> {
        topology.validate()?;
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
                return self.sync_directory();
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
        self.sync_directory()
    }

    fn sync_directory(&self) -> Result<()> {
        require_directory_durability()?;
        #[cfg(unix)]
        for directory in self.directory.ancestors() {
            fs::File::open(directory)?.sync_all()?;
        }
        Ok(())
    }
}

fn require_directory_durability() -> Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "persistent agent state requires Unix directory durability",
        )))
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
    #[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
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
    #[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
    fn oversized_snapshot_cannot_publish_or_advance_the_controller() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let mut initial = config();
        let store = Store::open(directory.clone(), &mut initial)?;
        let controller = crate::Controller::new(initial.topology.clone())?;
        controller.bind_store(store.clone())?;
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
        assert!(matches!(controller.apply(&plan), Err(Error::MessageTooLarge)));
        assert_eq!(controller.topology(), initial.topology);
        assert!(!directory.join("00000000000000000001.json").exists());
        assert_eq!(
            fs::read_dir(&directory)?.count(),
            2,
            "only initial revision and its ownership lock"
        );
        drop(controller);
        drop(store);
        let _restored = Store::open(directory, &mut initial)?;
        assert_eq!(initial.topology.revision, 0);
        Ok(())
    }

    #[test]
    #[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
    fn higher_edited_enrollment_cannot_override_committed_withdrawal() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let mut initial = config();
        let store = Store::open(directory.clone(), &mut initial)?;
        let mut withdrawn = initial.topology.clone();
        withdrawn.revision = 1;
        withdrawn.nodes.clear();
        store.persist(&withdrawn)?;
        drop(store);
        let mut edited = config();
        edited.topology.revision = 999;
        edited.topology.services.insert(
            "ignored-orphan".into(),
            crate::Service {
                node: "missing".into(),
                port: 22,
            },
        );
        assert!(edited.topology.validate().is_err());
        let restored = Store::open(directory.clone(), &mut edited)?;
        assert_eq!(edited.topology, withdrawn);
        assert!(!directory.join("00000000000000000999.json").exists());
        drop(restored);
        let mut edited = config();
        edited.topology.revision = 1;
        let _restored = Store::open(directory, &mut edited)?;
        assert_eq!(edited.topology, withdrawn);
        Ok(())
    }

    #[test]
    #[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
    fn invalid_initial_enrollment_cannot_publish_a_snapshot() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let mut invalid = config();
        invalid.topology.services.insert(
            "orphan".into(),
            crate::Service {
                node: "missing".into(),
                port: 22,
            },
        );
        assert!(matches!(
            Store::open(directory.clone(), &mut invalid),
            Err(Error::InvalidTopology(_))
        ));
        assert_eq!(fs::read_dir(&directory)?.count(), 1, "only the ownership lock");
        let mut valid = config();
        let _store = Store::open(directory.clone(), &mut valid)?;
        assert!(directory.join("00000000000000000000.json").exists());
        assert_eq!(valid.topology, config().topology);
        Ok(())
    }

    #[test]
    #[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
    fn invalid_direct_write_preserves_the_last_snapshot_and_policy() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let mut initial = config();
        let store = Store::open(directory.clone(), &mut initial)?;
        let controller = crate::Controller::new(initial.topology.clone())?;
        controller.bind_store(store.clone())?;
        let original = fs::read(directory.join("00000000000000000000.json"))?;
        let mut invalid = initial.topology.clone();
        invalid.revision = 1;
        invalid.nodes.clear();
        invalid.services.insert(
            "orphan".into(),
            crate::Service {
                node: "node".into(),
                port: 22,
            },
        );
        assert!(matches!(store.persist(&invalid), Err(Error::InvalidTopology(_))));
        assert!(matches!(controller.plan(invalid), Err(Error::InvalidTopology(_))));
        assert_eq!(controller.topology(), initial.topology);
        assert_eq!(fs::read(directory.join("00000000000000000000.json"))?, original);
        assert_eq!(
            fs::read_dir(&directory)?.count(),
            2,
            "no invalid snapshot or temporary file"
        );
        drop(controller);
        drop(store);
        let mut enrollment = config();
        let _restored = Store::open(directory, &mut enrollment)?;
        assert_eq!(enrollment.topology, initial.topology);
        Ok(())
    }

    #[test]
    #[cfg_attr(windows, ignore = "Persistent agent state requires Unix directory durability")]
    fn committed_corrupt_state_fails_closed() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("state");
        let store = Store::open(directory.clone(), &mut config())?;
        write_private(&directory.join("00000000000000000001.json"), b"invalid json")?;
        drop(store);
        assert!(Store::open(directory, &mut config()).is_err());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn unsupported_persistent_state_preserves_existing_files_and_never_takes_ownership() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        for existing in [false, true] {
            let directory = temporary
                .path()
                .join(if existing { "existing" } else { "absent/child" });
            if existing {
                fs::create_dir(&directory)?;
                fs::write(directory.join("agent.lock"), b"owned fixture")?;
                fs::write(
                    directory.join("00000000000000000001.json"),
                    b"malformed retained snapshot",
                )?;
            }
            let mut initial = config();
            let original = serde_json::to_value(&initial)?;
            assert!(matches!(
                Store::open(directory.clone(), &mut initial),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported
            ));
            assert_eq!(serde_json::to_value(&initial)?, original);
            if existing {
                assert_eq!(fs::read_dir(&directory)?.count(), 2);
                assert_eq!(fs::read(directory.join("agent.lock"))?, b"owned fixture");
                assert_eq!(
                    fs::read(directory.join("00000000000000000001.json"))?,
                    b"malformed retained snapshot"
                );
                let ownership = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(directory.join("agent.lock"))?;
                assert!(ownership.try_lock().is_ok());
                ownership.unlock()?;
            } else {
                assert!(!directory.exists());
                assert!(!directory.parent().unwrap().exists());
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn published_withdrawal_retry_requires_directory_durability_before_live_commit() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        struct RestorePermissions(PathBuf);

        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
        }

        if rustix::process::geteuid().is_root() {
            return Ok(()); // Root bypasses the real directory permission boundary.
        }

        for revoke in [false, true] {
            let temporary = tempfile::tempdir()?;
            let directory = temporary.path().join("state");
            let mut initial = config();
            initial.topology.services.insert(
                "self-service".into(),
                crate::Service {
                    node: "node".into(),
                    port: 22,
                },
            );
            initial.topology.grants.insert(
                "lease".into(),
                crate::Grant {
                    from: vec!["node".into()],
                    to: "self-service".into(),
                    expires_at: u64::MAX,
                },
            );
            let store = Store::open(directory.clone(), &mut initial)?;
            let controller = crate::Controller::new(initial.topology.clone())?;
            controller.bind_store(store)?;
            let mut withdrawn = initial.topology.clone();
            withdrawn.revision = 1;
            withdrawn.grants.clear();
            let plan = controller.plan(withdrawn.clone())?;
            let withdraw = || -> Result<()> {
                if revoke {
                    assert!(controller.revoke("lease")?);
                } else {
                    assert!(controller.apply(&plan)?.changed);
                }
                Ok(())
            };

            let restore = RestorePermissions(directory.clone());
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o300))?;
            for _ in 0..2 {
                assert!(
                    matches!(withdraw(), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied)
                );
                assert_eq!(controller.topology(), initial.topology);
                assert!(directory.join("00000000000000000001.json").is_file());
                assert!(fs::File::open(&directory).is_err());
            }

            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            drop(restore);
            withdraw()?;
            assert_eq!(controller.topology(), withdrawn);
            drop(controller);
            let mut enrollment = config();
            let _restored = Store::open(directory, &mut enrollment)?;
            assert_eq!(enrollment.topology, withdrawn);
        }
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

    // These tests require Unix permission modes, directory fsync, and symbolic links.
    #[cfg(unix)]
    struct RestoreAncestorPermissions(PathBuf);

    #[cfg(unix)]
    impl Drop for RestoreAncestorPermissions {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
        }
    }

    #[cfg(unix)]
    fn relative_from_current_directory(absolute: &Path) -> Result<PathBuf> {
        let mut relative = PathBuf::new();
        for component in std::env::current_dir()?.components() {
            if matches!(component, std::path::Component::Normal(_)) {
                relative.push("..");
            }
        }
        for component in absolute.components() {
            if let std::path::Component::Normal(part) = component {
                relative.push(part);
            }
        }
        Ok(relative)
    }

    #[cfg(unix)]
    #[test]
    fn initial_nested_directory_publication_and_visible_retry_require_ancestor_durability() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return Ok(()); // Root bypasses the real directory permission boundary.
        }
        let working_directory = std::env::current_dir()?;
        for relative in [false, true] {
            let temporary = tempfile::tempdir()?;
            let ancestor = temporary.path().join("owned-ancestor");
            fs::create_dir(&ancestor)?;
            let directory = ancestor.join("new-parent/new-child/state");
            let requested = if relative {
                relative_from_current_directory(&directory)?
            } else {
                directory.clone()
            };
            let restore = RestoreAncestorPermissions(ancestor.clone());
            fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o300))?;
            let mut initial = config();
            assert!(matches!(
                Store::open(requested.clone(), &mut initial),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
            ));
            let published = directory.join("00000000000000000000.json");
            let original = fs::read(&published)?;
            assert!(matches!(
                Store::open(requested.clone(), &mut initial),
                Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
            ));
            assert_eq!(fs::read(&published)?, original);
            assert_eq!(std::env::current_dir()?, working_directory);
            fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))?;
            drop(restore);
            let store = Store::open(requested, &mut initial)?;
            assert_eq!(store.directory, directory.canonicalize()?);
            assert!(store.directory.is_absolute());
            assert_eq!(initial.topology, config().topology);
            drop(store);
        }
        assert_eq!(std::env::current_dir()?, working_directory);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn published_withdrawal_requires_ancestor_barrier_on_every_identical_retry() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            return Ok(()); // Root bypasses the real directory permission boundary.
        }
        for revoke in [false, true] {
            let temporary = tempfile::tempdir()?;
            let ancestor = temporary.path().join("owned-ancestor");
            let directory = ancestor.join("nested/state");
            let mut initial = config();
            initial.topology.services.insert(
                "self-service".into(),
                crate::Service {
                    node: "node".into(),
                    port: 22,
                },
            );
            initial.topology.grants.insert(
                "lease".into(),
                crate::Grant {
                    from: vec!["node".into()],
                    to: "self-service".into(),
                    expires_at: u64::MAX,
                },
            );
            let source_key = initial.topology.nodes["node"].key.clone();
            let store = Store::open(directory.clone(), &mut initial)?;
            let controller = crate::Controller::new(initial.topology.clone())?;
            controller.bind_store(store)?;
            let mut withdrawn = initial.topology.clone();
            withdrawn.revision = 1;
            withdrawn.grants.clear();
            let plan = controller.plan(withdrawn.clone())?;
            let withdraw = || -> Result<()> {
                if revoke {
                    assert!(controller.revoke("lease")?);
                } else {
                    assert!(controller.apply(&plan)?.changed);
                }
                Ok(())
            };
            let restore = RestoreAncestorPermissions(ancestor.clone());
            fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o300))?;
            for _ in 0..2 {
                assert!(matches!(
                    withdraw(),
                    Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::PermissionDenied
                ));
                assert_eq!(controller.topology(), initial.topology);
                assert!(controller.authorize(&source_key, "self-service").is_ok());
                assert!(directory.join("00000000000000000001.json").is_file());
                assert!(fs::File::open(&directory)?.sync_all().is_ok());
                assert!(fs::File::open(&ancestor).is_err());
            }
            fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700))?;
            drop(restore);
            withdraw()?;
            assert_eq!(controller.topology(), withdrawn);
            assert!(matches!(
                controller.authorize(&source_key, "self-service"),
                Err(Error::Denied)
            ));
            drop(controller);
            let mut enrollment = config();
            let _restored = Store::open(directory, &mut enrollment)?;
            assert_eq!(enrollment.topology, withdrawn);
            assert!(matches!(
                crate::Controller::new(enrollment.topology)?.authorize(&source_key, "self-service"),
                Err(Error::Denied)
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn state_directory_uses_physical_ancestry_through_a_parent_symlink() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let actual = temporary.path().join("actual");
        fs::create_dir(&actual)?;
        let alias = temporary.path().join("alias");
        std::os::unix::fs::symlink(&actual, &alias)?;
        let working_directory = std::env::current_dir()?;
        let requested = relative_from_current_directory(&alias.join("nested/state"))?;
        let mut initial = config();
        let store = Store::open(requested, &mut initial)?;
        assert_eq!(store.directory, actual.join("nested/state").canonicalize()?);
        assert!(store.directory.is_absolute());
        assert!(!store.directory.starts_with(&alias));
        assert_eq!(std::env::current_dir()?, working_directory);
        assert!(store.directory.join("00000000000000000000.json").is_file());
        drop(store);
        Ok(())
    }
}
