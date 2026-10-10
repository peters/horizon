//! Reject extended macOS ACLs; BSD owner-only mode bits are not sufficient.
use super::{Error, Result};

use std::{
    io::{Read as _, Result as IoResult},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_OUTPUT: u64 = 64 * 1024;

#[cfg(target_os = "macos")]
pub(super) fn protect(path: &Path) -> Result<()> {
    run("/bin/chmod", &["-N"], path)?;
    verify(path)
}

#[cfg(target_os = "macos")]
pub(super) fn verify(path: &Path) -> Result<()> {
    // Numeric identities avoid name-service lookups; escaped paths cannot forge
    // an extra output line. Extended attributes can hide the '+' mode marker,
    // so inspect both the marker and the complete ACL listing.
    acl_free_listing(&run("/bin/ls", &["-ldneB"], path)?)
}

fn acl_free_listing(bytes: &[u8]) -> Result<()> {
    let output =
        std::str::from_utf8(bytes).map_err(|_| Error::Invalid("macOS credential permissions could not be verified"))?;
    let mut lines = output.lines();
    let header = lines.next().unwrap_or_default().as_bytes();
    if !matches!(header.first(), Some(b'd' | b'-'))
        || !matches!(header.get(10), Some(b' ' | b'@'))
        || lines.next().is_some()
    {
        return Err(Error::Invalid("macOS credential objects must have no extended ACL"));
    }
    Ok(())
}

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn reader(stream: impl std::io::Read + Send + 'static) -> Result<JoinHandle<IoResult<Vec<u8>>>> {
    Ok(thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        stream.take(MAX_OUTPUT + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    })?)
}

fn run(executable: &str, arguments: &[&str], path: &Path) -> Result<Vec<u8>> {
    // An absolute argument cannot be interpreted as another command option.
    let path = std::path::absolute(path)?;
    let mut child = OwnedChild(
        Command::new(executable)
            .args(arguments)
            .arg(path)
            .env("LC_ALL", "C")
            .env_remove("CLICOLOR_FORCE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let stdout = reader(
        child
            .0
            .stdout
            .take()
            .ok_or(Error::Invalid("missing permission output"))?,
    )?;
    let stderr = reader(
        child
            .0
            .stderr
            .take()
            .ok_or(Error::Invalid("missing permission diagnostics"))?,
    )?;
    let status = wait(&mut child.0)?;
    let output = stdout
        .join()
        .map_err(|_| Error::Invalid("macOS permission reader failed"))??;
    let diagnostics = stderr
        .join()
        .map_err(|_| Error::Invalid("macOS permission reader failed"))??;
    if !status.success() || !diagnostics.is_empty() || output.len() as u64 > MAX_OUTPUT {
        return Err(Error::Invalid("macOS credential permissions could not be verified"));
    }
    Ok(output)
}

fn wait(child: &mut Child) -> Result<ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            return Err(Error::Invalid("macOS credential permission verification timed out"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_listing_refuses_acls_and_unrecognized_output() {
        for output in [
            "",
            "unknown\n",
            "lrwx------ 1 501 20 8 Jan 1 00:00 record\n",
            "drwx------+ 1 501 20 0 Jan 1 00:00 store\n 0: user:nobody allow list,search\n",
            "-rw-------@ 1 501 20 1 Jan 1 00:00 record\n 0: user:nobody allow read\n",
        ] {
            assert!(acl_free_listing(output.as_bytes()).is_err(), "{output:?}");
        }
        for output in [
            "drwx------ 1 501 20 0 Jan 1 00:00 store\n",
            "-rw-------@ 1 501 20 1 Jan 1 00:00 escaped\\012name\n",
        ] {
            assert!(acl_free_listing(output.as_bytes()).is_ok(), "{output:?}");
        }
    }

    #[test]
    fn permission_command_bounds_output_and_refuses_failed_or_incomplete_reads() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            run("/bin/sh", &["-c", "printf 'bounded metadata'"], root.path()).unwrap(),
            b"bounded metadata"
        );
        for script in [
            "exit 1",
            "printf 'metadata'; printf 'ACL unavailable' >&2",
            r#"i=0; while [ "$i" -lt 4097 ]; do printf '0123456789abcdef'; i=$((i+1)); done"#,
        ] {
            assert!(run("/bin/sh", &["-c", script], root.path()).is_err());
        }
    }

    #[cfg(target_os = "macos")]
    fn add_acl(path: &Path, entry: &str) {
        run("/bin/chmod", &["+a", entry], path).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn directory_acl_is_refused_on_reads_and_removed_before_writes() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let directory = super::super::directory(root.path());
        let path = directory.join("synthetic-record");
        super::super::private_file(&path, b"synthetic").unwrap();
        add_acl(&directory, "user:nobody allow list,search,readattr,readsecurity");
        assert_eq!(std::fs::metadata(&directory).unwrap().permissions().mode() & 0o077, 0);
        assert!(super::super::read_private(&path).is_err());
        assert!(super::super::connections(root.path()).is_err());
        assert!(super::super::super::status(root.path()).is_err());
        super::super::private_file(&path, b"updated synthetic").unwrap();
        assert!(verify(&directory).is_ok());
        assert_eq!(
            &*super::super::read_private(&path).unwrap().unwrap(),
            b"updated synthetic"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn file_acl_is_refused_and_atomic_replacement_has_no_acl() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let path = super::super::directory(root.path()).join("synthetic-record");
        super::super::private_file(&path, b"synthetic").unwrap();
        add_acl(&path, "user:nobody allow read,readattr,readsecurity");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o077, 0);
        assert!(super::super::read_private(&path).is_err());
        super::super::private_file(&path, b"replacement").unwrap();
        assert!(verify(&path).is_ok());
        assert_eq!(&*super::super::read_private(&path).unwrap().unwrap(), b"replacement");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn inherited_acls_are_removed_from_new_credential_objects() {
        let root = tempfile::tempdir().unwrap();
        add_acl(
            root.path(),
            "user:nobody allow list,search,read,file_inherit,directory_inherit",
        );
        let directory = super::super::directory(root.path());
        let path = directory.join("synthetic-record");
        super::super::private_file(&path, b"synthetic").unwrap();
        assert!(verify(&directory).is_ok());
        assert!(verify(&path).is_ok());
        assert_eq!(&*super::super::read_private(&path).unwrap().unwrap(), b"synthetic");
    }
}
