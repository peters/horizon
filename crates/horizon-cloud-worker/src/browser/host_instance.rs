//! Publishes the browser host instance to the worker's unprivileged agent sessions.
//!
//! The control service runs as root and keeps its browser runtime root private, so
//! agents cannot read the value there. It is an identity, not a credential: the
//! launcher forwards it to the browser MCP of each agent session.
use std::{io, path::Path};

/// Inside the supervisor's runtime directory, which root owns and agents can only read.
pub const PUBLISHED: &str = "/run/horizon-worker/browser-host-instance";

/// Atomically replaces `path` with a world-readable copy of `value`. The directory
/// must belong to this process and refuse writes by others, so no agent can redirect
/// or replace the file.
pub fn publish(path: &Path, value: &str) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Browser host instance has no directory"))?;
    check_directory(directory)?;
    let pending = path.with_extension("new");
    write_readable(&pending, value)?;
    std::fs::rename(&pending, path)
}

#[cfg(unix)]
fn check_directory(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
        return Err(io::Error::other(
            "Browser host instance directory must be owned by the worker service and not writable by others",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_directory(directory: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(directory)?.is_dir() {
        Ok(())
    } else {
        Err(io::Error::other("Browser host instance directory is missing"))
    }
}

fn write_readable(path: &Path, value: &str) -> io::Result<()> {
    std::fs::write(path, format!("{value}\n"))?;
    // The service runs with umask 077; the published copy must be readable by agents.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Like the supervisor's runtime directory: owned by the service, mode 0755.
    fn runtime_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        directory
    }

    #[test]
    fn publishes_a_readable_copy_and_replaces_an_older_one() {
        let directory = runtime_directory();
        let path = directory.path().join("browser-host-instance");
        publish(&path, "older-host").unwrap();
        publish(&path, "current-host").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "current-host\n");
        assert!(!path.with_extension("new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o644, "agents read it under the service's restrictive umask");
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_directory_that_others_can_write() {
        use std::os::unix::fs::PermissionsExt;
        let directory = runtime_directory();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let path = directory.path().join("browser-host-instance");
        assert!(publish(&path, "host").is_err());
        assert!(!path.exists());
        assert!(!path.with_extension("new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_directory() {
        use std::os::unix::fs::PermissionsExt;
        let directory = runtime_directory();
        let real = directory.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        publish(&real.join("browser-host-instance"), "real-host").unwrap();
        std::fs::remove_file(real.join("browser-host-instance")).unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(publish(&link.join("browser-host-instance"), "host").is_err());
        assert!(!real.join("browser-host-instance").exists());
    }
}
