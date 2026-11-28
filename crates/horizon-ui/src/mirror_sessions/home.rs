//! The home of the publisher of the session mirror: one private folder of this
//! computer, which holds the name that the server gave it, the address of the
//! server and the token of its join.
//!
//! `horizon --mirror-sessions` reads and writes no `~/.horizon`: this folder is
//! `HORIZON_MIRROR_HOME` or `~/.horizon-mirror`, at mode 0700, and each file in it
//! is at mode 0600. The token is kept by the secrets store of Horizon, at the
//! `server/token` id of the folder, as the token of a computer is.

use std::path::{Path, PathBuf};

use horizon_core::AtomicFile;
use horizon_core::secret_store::SecretStore;

/// The mode of the folder of this home.
const DIRECTORY_MODE: u32 = 0o700;

/// The file of the name that the server gave this publisher.
const DEVICE_FILE: &str = "server-device";
/// The file of the address of the server, `host:port`, as the join link named it.
const ADDRESS_FILE: &str = "server-address";
/// The file of the token of the join.
const SECRETS_FILE: &str = "secrets.sqlite";

/// The home of one publisher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorHome {
    root: PathBuf,
}

impl MirrorHome {
    /// The home of this start: `HORIZON_MIRROR_HOME`, then `~/.horizon-mirror`.
    ///
    /// # Errors
    /// Returns why this start cannot name a home.
    pub fn resolve() -> Result<Self, String> {
        let named = std::env::var_os("HORIZON_MIRROR_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty());
        named
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".horizon-mirror"))
            })
            .map(Self::at)
            .ok_or_else(|| "the publisher needs a home: name HORIZON_MIRROR_HOME or set HOME".to_owned())
    }

    /// The home at `root`.
    #[must_use]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// The folder of this home.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Makes the folder at mode 0700, and leaves a folder that is there.
    ///
    /// # Errors
    /// Returns why the folder cannot be made private.
    pub fn prepare(&self) -> Result<(), String> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(DIRECTORY_MODE);
        }
        builder
            .create(&self.root)
            .and_then(|()| private_directory(&self.root))
            .map_err(|error| format!("the folder {}: {error}", self.root.display()))
    }

    /// The secrets store of this home, whose token is the token of the join.
    ///
    /// # Errors
    /// Returns why the store cannot be opened.
    pub fn secrets(&self) -> Result<std::sync::Arc<SecretStore>, String> {
        SecretStore::open(self.root.join(SECRETS_FILE)).map_err(|error| format!("the secrets of this home: {error}"))
    }

    /// The name that the server gave this publisher.
    #[must_use]
    pub fn device(&self) -> Option<String> {
        let text = self.file(DEVICE_FILE).read().ok().flatten()?;
        let name = text.trim().to_owned();
        horizon_sync::check_device_name(&name).then_some(name)
    }

    /// Writes the name that the server gave this publisher.
    ///
    /// # Errors
    /// Returns why the file cannot be written.
    pub fn write_device(&self, device: &str) -> Result<(), String> {
        self.write(DEVICE_FILE, device)
    }

    /// The address of the server, `host:port`.
    #[must_use]
    pub fn address(&self) -> Option<String> {
        let text = self.file(ADDRESS_FILE).read().ok().flatten()?;
        let address = text.trim().to_owned();
        (!address.is_empty()).then_some(address)
    }

    /// Writes the address of the server.
    ///
    /// # Errors
    /// Returns why the file cannot be written.
    pub fn write_address(&self, address: &str) -> Result<(), String> {
        self.write(ADDRESS_FILE, address)
    }

    fn file(&self, name: &str) -> AtomicFile {
        AtomicFile::new(self.root.join(name))
    }

    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        let file = self.file(name);
        file.write(value.as_bytes())
            .map_err(|error| format!("the file {}: {error}", file.path().display()))
    }
}

/// Makes a folder that is there private.
fn private_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(DIRECTORY_MODE))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::MirrorHome;

    #[test]
    fn the_home_is_private_and_holds_the_name_and_the_address() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = MirrorHome::at(directory.path().join("mirror"));
        home.prepare().expect("the home");
        assert_eq!(
            std::fs::metadata(home.root()).expect("the home").permissions().mode() & 0o777,
            0o700,
            "the home of the publisher is the person's alone"
        );

        home.write_device("laptop-sessions").expect("the name");
        home.write_address("127.0.0.1:7000").expect("the address");
        assert_eq!(home.device().as_deref(), Some("laptop-sessions"));
        assert_eq!(home.address().as_deref(), Some("127.0.0.1:7000"));
        for name in ["server-device", "server-address"] {
            assert_eq!(
                std::fs::metadata(home.root().join(name))
                    .expect("the file")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "{name} is the person's alone"
            );
        }
    }

    #[test]
    fn a_name_that_breaks_the_rule_of_a_device_is_no_name() {
        let directory = tempfile::tempdir().expect("a directory");
        let home = MirrorHome::at(directory.path().to_path_buf());
        home.prepare().expect("the home");
        assert_eq!(home.device(), None, "a home without a file has no name");
        home.write_device("Laptop Sessions").expect("the file");
        assert_eq!(home.device(), None, "a name with a capital and a space is no name");
    }
}
