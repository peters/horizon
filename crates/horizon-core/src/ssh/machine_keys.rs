//! The machine key pairs that Horizon keeps for the machines an agent rents.
//!
//! An agent asks for a key by name, puts the public line on the machine at rent
//! time, and asks Horizon to open a terminal with that key. The pair belongs to
//! Horizon, in its home, so a rented machine never holds the person's own key
//! and the login does not fail because an agent offers more keys than the
//! server's `MaxAuthTries` allows. The keys belong to the Horizon home and not
//! to a workspace, because the machines belong to the person and not to one
//! session: every agent panel of this Horizon may use a key by name.
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
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

use horizon_browser_control::manifest::block_keys;
use serde::{Deserialize, Serialize};

use crate::secret::MachineKeyFiles;

/// The mode of the directory that holds the keys of this Horizon home.
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
            identity_option(&self.private_key()),
            ("-o", "IdentitiesOnly=yes".to_string()),
            ("-o", "IdentityAgent=none".to_string()),
            ("-o", format!("UserKnownHostsFile={}", ssh_path(&self.known_hosts()))),
            ("-o", "StrictHostKeyChecking=accept-new".to_string()),
        ]
        .into_iter()
        .flat_map(|(option, value)| [option.to_string(), value])
        .collect()
    }
}

/// One key pair of this Horizon home, as an operation reports it.
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
    /// This Horizon home already holds the most keys that it may hold.
    #[error(
        "This Horizon home already holds the maximum of {limit} machine keys. Delete one with the delete_machine_key operation before you ask for another"
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

/// The machine keys of one Horizon home, in `machine-keys/<name>`.
///
/// The keys belong to the Horizon home and not to a workspace: every agent
/// panel of this Horizon may connect with any of them by name, and a key is
/// found again after a restart without a session or a workspace.
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

    /// The directory of this home's keys. It holds no `%` of its own: ssh reads
    /// `%` in a key path and in a known-hosts path as the start of a token, and
    /// an agent passes the path of its key to its own ssh as it is.
    #[must_use]
    pub fn key_directory(&self) -> PathBuf {
        self.root.clone()
    }

    /// The connection of a panel that uses the key `name`.
    #[must_use]
    pub fn binding(&self, name: &str) -> MachineKeyRef {
        MachineKeyRef {
            directory: self.key_directory().display().to_string(),
            name: name.to_owned(),
        }
    }

    /// The key pair of `name`, made now when it is not there.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, a home that already holds `limit`
    /// keys, a pair with only one half, and a missing or failing `ssh-keygen`.
    pub fn ensure(&self, name: &str, limit: usize) -> Result<MachineKey, MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let binding = self.binding(name);
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
        if self.list()?.len() >= limit {
            return Err(MachineKeyError::TooMany { limit });
        }
        private_directory(&self.key_directory())?;
        make_key_pair(&private, &public, name)?;
        fs::set_permissions(&private, fs::Permissions::from_mode(PRIVATE_KEY_MODE))
            .map_err(|error| not_usable(format!("Horizon could not make the key {name} private: {error}")))?;
        read_key(&binding, &public, true)
    }

    /// Every key pair of this home, by name.
    ///
    /// # Errors
    /// Returns why the key directory cannot be read.
    pub fn list(&self) -> Result<Vec<MachineKey>, MachineKeyError> {
        let directory = self.key_directory();
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
            let binding = self.binding(name);
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

    /// The connection of the existing key `name`, for a panel that connects
    /// with it.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, and a key that this Horizon home
    /// does not hold.
    pub fn require(&self, name: &str) -> Result<MachineKeyRef, MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let binding = self.binding(name);
        if !binding.private_key().is_file() {
            return Err(not_usable(format!(
                "This Horizon home has no machine key {name}; ask for it with the machine_key operation first"
            )));
        }
        Ok(binding)
    }

    /// The three files of the key `name`, or `None` when this home holds no
    /// private key of that name.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, and returns why a file cannot be
    /// read.
    pub fn files(&self, name: &str) -> Result<Option<MachineKeyFiles>, MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let binding = self.binding(name);
        let private = binding.private_key();
        let private_key = match fs::read_to_string(&private) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(not_usable(format!(
                    "Horizon could not read the machine key {name}: {error}"
                )));
            }
        };
        Ok(Some(MachineKeyFiles {
            private_key,
            public_key: read_or_empty(&binding.public_key(), name)?,
            known_hosts: read_or_empty(&binding.known_hosts(), name)?,
        }))
    }

    /// Writes the three files of the key `name` as `files` gives them, in a
    /// private directory, with a private key at mode 0600 whatever mode a file of
    /// that name had before.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, and returns why the directory or a
    /// file cannot be written.
    pub fn write_files(&self, name: &str, files: &MachineKeyFiles) -> Result<(), MachineKeyError> {
        let name = block_keys::check_name(name)?;
        private_directory(&self.key_directory())?;
        let binding = self.binding(name);
        for (path, text) in [
            (binding.private_key(), files.private_key.as_str()),
            (binding.public_key(), files.public_key.as_str()),
            (binding.known_hosts(), files.known_hosts.as_str()),
        ] {
            write_private(&path, text.as_bytes()).map_err(|error| {
                not_usable(format!(
                    "Horizon could not write {} of the machine key {name}: {error}",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    /// Delete the key pair of `name`, with its known-hosts file.
    ///
    /// # Errors
    /// Refuses a name that breaks the rule, a key that is not there, and a file
    /// that Horizon cannot remove.
    pub fn delete(&self, name: &str) -> Result<(), MachineKeyError> {
        let name = block_keys::check_name(name)?;
        let directory = self.key_directory();
        let paths = [
            directory.join(name),
            directory.join(format!("{name}.pub")),
            directory.join(format!("{name}.known_hosts")),
        ];
        if !paths.iter().any(|path| path.is_file()) {
            return Err(not_usable(format!("This Horizon home has no machine key {name}")));
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

/// The text of one optional file of a key. A file that is not there reads as
/// empty, because a key that never logged in has no known-hosts line yet.
fn read_or_empty(path: &Path, name: &str) -> Result<String, MachineKeyError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(not_usable(format!(
            "Horizon could not read {} of the machine key {name}: {error}",
            path.display()
        ))),
    }
}

/// Replaces one file of a key as a whole through a temporary file of its own
/// name and a rename, at mode 0600.
fn write_private(path: &Path, text: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let written = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PRIVATE_KEY_MODE)
        .open(&temporary)
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(text)?;
            file.sync_all()
        })
        .and_then(|()| fs::set_permissions(&temporary, fs::Permissions::from_mode(PRIVATE_KEY_MODE)))
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
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

/// A path as the value of an ssh configuration option. ssh expands `%` tokens
/// in `IdentityFile` and in `UserKnownHostsFile`, and `%%` is its literal
/// percent sign.
fn ssh_path(path: &Path) -> String {
    path.display().to_string().replace('%', "%%")
}

/// The option that names the key file.
///
/// `-i` is the plain form. ssh looks for the file of `-i` before it expands
/// tokens and expands them afterwards, so a path with a percent sign works in
/// neither spelling there; the `IdentityFile` option takes the doubled sign.
fn identity_option(private_key: &Path) -> (&'static str, String) {
    let plain = private_key.display().to_string();
    if plain.contains('%') {
        ("-o", format!("IdentityFile={}", ssh_path(private_key)))
    } else {
        ("-i", plain)
    }
}

#[cfg(test)]
mod tests;
