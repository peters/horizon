//! The machine key pairs that Horizon keeps for the machines an agent rents.
//!
//! An agent asks for a key by name, puts the public line on the machine at rent
//! time, and asks Horizon to open a terminal with that key. The pair belongs to
//! Horizon, in its home, so a rented machine never holds the person's own key
//! and the login does not fail because an agent offers more keys than the
//! server's `MaxAuthTries` allows.
//!
//! Every call here reads or writes files and starts `ssh-keygen`, so it runs on
//! a coordination worker thread and never on the UI thread. The private half
//! stays in this directory: its text is written to no answer, log line, card or
//! error message, and the only text that leaves the directory is the public
//! line together with the names of the two files. An agent runs as the same user
//! as Horizon and can read the files anyway, so it receives their paths and may
//! run its own `ssh` with them.

use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

use horizon_browser_control::manifest::block_keys;
use horizon_browser_control::paths::safe_local_id;
use serde::{Deserialize, Serialize};

/// The mode of the directory that holds one workspace's keys.
const DIRECTORY_MODE: u32 = 0o700;

/// The mode of a private key. OpenSSH refuses a key that another user can read.
const PRIVATE_KEY_MODE: u32 = 0o600;

/// The connection of a panel that uses one machine key.
///
/// Horizon saves the directory with the key name, so a restored panel builds
/// the same command line and an agent never supplies a path. The directory is
/// Horizon's own value from [`MachineKeys`], not request text.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct MachineKeyRef {
    /// The directory that holds the key pair and its known-hosts file.
    pub directory: String,
    /// The name that the agent gave the key.
    pub name: String,
}

impl MachineKeyRef {
    /// The private half, which `ssh -i` reads.
    #[must_use]
    pub fn private_key(&self) -> PathBuf {
        Path::new(&self.directory).join(&self.name)
    }

    /// The public half that the agent puts on the machine.
    #[must_use]
    pub fn public_key(&self) -> PathBuf {
        Path::new(&self.directory).join(format!("{}.pub", self.name))
    }

    /// The known-hosts file of this key alone, so a recycled address never
    /// collides with another key or with the person's own known hosts.
    #[must_use]
    pub fn known_hosts(&self) -> PathBuf {
        Path::new(&self.directory).join(format!("{}.known_hosts", self.name))
    }

    /// The ssh options that use this key and no other.
    ///
    /// `IdentitiesOnly` and `IdentityAgent=none` keep the person's ssh agent out
    /// of the login, so the server sees one key. `accept-new` pins the host key
    /// on first contact and refuses a changed one afterwards.
    #[must_use]
    pub fn ssh_option_args(&self) -> Vec<String> {
        [
            ("-i", self.private_key().display().to_string()),
            ("-o", "IdentitiesOnly=yes".to_string()),
            ("-o", "IdentityAgent=none".to_string()),
            ("-o", format!("UserKnownHostsFile={}", self.known_hosts().display())),
            ("-o", "StrictHostKeyChecking=accept-new".to_string()),
        ]
        .into_iter()
        .flat_map(|(option, value)| [option.to_string(), value])
        .collect()
    }
}

/// One key pair of a workspace, as an operation reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineKey {
    pub name: String,
    /// The public line only, ready for `authorized_keys` on the machine.
    pub public_key: String,
    /// The private half, as an absolute path. The agent may read the file and
    /// run its own `ssh -i` with it for the commands that set the machine up;
    /// the text of the key is in no answer.
    pub private_key_path: String,
    /// The known-hosts file of this key alone, as an absolute path.
    pub known_hosts_path: String,
    pub created_at_millis: u64,
    /// Whether this request made the pair.
    pub created: bool,
}

/// Why Horizon could not make, read or delete a machine key.
#[derive(Debug, thiserror::Error)]
pub enum MachineKeyError {
    /// The name is not one key label.
    #[error("{message}")]
    InvalidName { message: String },
    /// The workspace already holds the most keys that it may hold.
    #[error(
        "This workspace already holds the maximum of {limit} machine keys. Delete one with the delete_machine_key operation before you ask for another"
    )]
    TooMany { limit: usize },
    /// The key pair is not there, or Horizon could not make or delete it.
    #[error("{reason}")]
    Unavailable { reason: String },
}

impl MachineKeyError {
    /// The refusal code that the tool reports for this error.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidName { .. } => block_keys::INVALID_KEY_NAME,
            Self::TooMany { .. } => block_keys::TOO_MANY_KEYS,
            Self::Unavailable { .. } => block_keys::KEY_UNAVAILABLE,
        }
    }
}

impl From<block_keys::KeyRefusal> for MachineKeyError {
    fn from(refusal: block_keys::KeyRefusal) -> Self {
        Self::InvalidName {
            message: refusal.message,
        }
    }
}

/// The machine keys of one Horizon home, in `machine-keys/<workspace>/<name>`.
#[derive(Clone, Debug)]
pub struct MachineKeys {
    root: PathBuf,
}

impl MachineKeys {
    /// The keys of `home`.
    #[must_use]
    pub fn from_home(home: &crate::horizon_home::HorizonHome) -> Self {
        Self::new(home.machine_keys_dir())
    }

    /// The keys of a directory that holds them, for a caller that resolved one.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The directory of one workspace's keys. The workspace is its saved local
    /// id, so the same workspace finds its keys after a restart.
    #[must_use]
    pub fn workspace_directory(&self, workspace: &str) -> PathBuf {
        self.root.join(safe_local_id(workspace))
    }

    /// The connection of a panel that uses the key `name` of `workspace`.
    #[must_use]
    pub fn binding(&self, workspace: &str, name: &str) -> MachineKeyRef {
        MachineKeyRef {
            directory: self.workspace_directory(workspace).display().to_string(),
            name: name.to_owned(),
        }
    }

    /// The key pair of `name` in `workspace`, made now when it is not there.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, a workspace that already holds
    /// `limit` keys, a pair with only one half, and a missing or failing
    /// `ssh-keygen`.
    pub fn ensure(&self, workspace: &str, name: &str, limit: usize) -> Result<MachineKey, MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let binding = self.binding(workspace, name);
        let (private, public) = (binding.private_key(), binding.public_key());
        match (private.is_file(), public.is_file()) {
            (true, true) => return read_key(&binding, &public, false),
            (false, false) => {}
            _ => {
                return Err(not_usable(format!(
                    "The machine key {name} is incomplete; delete it with the delete_machine_key operation and ask for it again"
                )));
            }
        }
        if self.list(workspace)?.len() >= limit {
            return Err(MachineKeyError::TooMany { limit });
        }
        private_directory(&self.workspace_directory(workspace))?;
        make_key_pair(&private, &public, name)?;
        fs::set_permissions(&private, fs::Permissions::from_mode(PRIVATE_KEY_MODE))
            .map_err(|error| not_usable(format!("Horizon could not make the key {name} private: {error}")))?;
        read_key(&binding, &public, true)
    }

    /// Every key pair of `workspace`, by name.
    ///
    /// # Errors
    /// Returns why the directory of the workspace cannot be read.
    pub fn list(&self, workspace: &str) -> Result<Vec<MachineKey>, MachineKeyError> {
        let directory = self.workspace_directory(workspace);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(not_usable(format!("Horizon could not read the machine keys: {error}")));
            }
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| not_usable(format!("Horizon could not read a machine key: {error}")))?;
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().and_then(|name| name.strip_suffix(".pub")) else {
                continue;
            };
            if block_keys::check_name(name).is_err() {
                continue;
            }
            let binding = self.binding(workspace, name);
            if !binding.private_key().is_file() {
                continue;
            }
            if let Ok(key) = read_key(&binding, &entry.path(), false) {
                keys.push(key);
            }
        }
        keys.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(keys)
    }

    /// The connection of the existing key `name` of `workspace`, for a panel
    /// that connects with it.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, and a key that this workspace does
    /// not hold.
    pub fn require(&self, workspace: &str, name: &str) -> Result<MachineKeyRef, MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let binding = self.binding(workspace, name);
        if !binding.private_key().is_file() {
            return Err(not_usable(format!(
                "This workspace has no machine key {name}; ask for it with the machine_key operation first"
            )));
        }
        Ok(binding)
    }

    /// Delete the key pair of `name` in `workspace`, with its known-hosts file.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, a key that is not there, and a file
    /// that Horizon cannot remove.
    pub fn delete(&self, workspace: &str, name: &str) -> Result<(), MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let directory = self.workspace_directory(workspace);
        let paths = [
            directory.join(name),
            directory.join(format!("{name}.pub")),
            directory.join(format!("{name}.known_hosts")),
        ];
        if !paths.iter().any(|path| path.is_file()) {
            return Err(not_usable(format!("This workspace has no machine key {name}")));
        }
        for path in &paths {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(not_usable(format!(
                        "Horizon could not delete the machine key {name}: {error}"
                    )));
                }
            }
        }
        Ok(())
    }
}

fn not_usable(reason: String) -> MachineKeyError {
    MachineKeyError::Unavailable { reason }
}

/// Read the public half of one pair. The public line and the two file names
/// leave the directory; the private key text never does.
fn read_key(binding: &MachineKeyRef, public: &Path, created: bool) -> Result<MachineKey, MachineKeyError> {
    let name = binding.name.as_str();
    let public_key = fs::read_to_string(public).map_err(|error| {
        not_usable(format!(
            "Horizon could not read the public key of the machine key {name}: {error}"
        ))
    })?;
    let private = binding.private_key();
    Ok(MachineKey {
        name: name.to_owned(),
        public_key: public_key.trim().to_owned(),
        private_key_path: private.display().to_string(),
        known_hosts_path: binding.known_hosts().display().to_string(),
        created_at_millis: created_at_millis(&private),
        created,
    })
}

/// Make `directory` exist and stay readable and writable by its owner alone.
fn private_directory(directory: &Path) -> Result<(), MachineKeyError> {
    let refused = |error: io::Error| {
        not_usable(format!(
            "Horizon could not make {} private: {error}",
            directory.display()
        ))
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIRECTORY_MODE)
        .create(directory)
        .map_err(refused)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(DIRECTORY_MODE)).map_err(refused)
}

/// Make an ed25519 pair with no passphrase, as Horizon's own key.
fn make_key_pair(private: &Path, public: &Path, name: &str) -> Result<(), MachineKeyError> {
    let output = Command::new("ssh-keygen")
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            &format!("horizon-machine-{name}"),
            "-f",
        ])
        .arg(private)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(not_usable(
                "Horizon could not find ssh-keygen, so it cannot make a machine key".to_owned(),
            ));
        }
        Err(error) => {
            return Err(not_usable(format!(
                "Horizon could not run ssh-keygen to make the machine key {name}: {error}"
            )));
        }
    };
    if output.status.success() && private.is_file() && public.is_file() {
        return Ok(());
    }
    // A failed run can leave half a pair behind. The caller asked for a name
    // that was not there, so nothing older is lost.
    let _ = fs::remove_file(private);
    let _ = fs::remove_file(public);
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(not_usable(if reason.is_empty() {
        format!("Horizon could not make the machine key {name}")
    } else {
        format!("Horizon could not make the machine key {name}: {reason}")
    }))
}

fn created_at_millis(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
