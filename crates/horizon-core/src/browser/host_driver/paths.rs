//! The folders that the driver of a browser host uses on that machine.
//!
//! Everything of a host driver is on the host: its profile, its downloads and
//! the socket that the next link attaches to. This computer never reads or
//! prepares one of them, which is the reason for a browser host in the first
//! place: the signed-in profile of that machine stays where it is.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The environment name of the home of the person, as `sh` sets it.
const HOME: &str = "HOME";

/// The folder of a browser host, under the home of the person of that machine.
pub(super) const HORIZON_FOLDER: &str = ".horizon";
/// The folder of the browser profiles of that home.
pub(super) const PROFILES_FOLDER: &str = "browser-profiles";

/// The longest socket path that a Unix socket accepts, in bytes.
const MAX_SOCKET_PATH_BYTES: usize = 100;

/// The home folder of the person of this machine, or the refusal that names the
/// missing variable.
///
/// # Errors
/// `HOME` is unset or empty.
pub(super) fn home() -> Result<PathBuf> {
    std::env::var_os(HOME)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::Config("the browser host driver needs HOME to find the profile of its browser".into()))
}

/// The folder of the browser profiles of this machine.
///
/// # Errors
/// See [`home`].
pub(super) fn profiles_root() -> Result<PathBuf> {
    Ok(home()?.join(HORIZON_FOLDER).join(PROFILES_FOLDER))
}

/// The folder of the named profile on this machine, which the browser of this
/// host fills and no other machine reads.
///
/// # Errors
/// See [`home`].
pub(super) fn profile_root(profile: &str) -> Result<PathBuf> {
    Ok(profiles_root()?.join(profile))
}

/// The socket of the session of `profile` and `engine` on this machine.
///
/// One session per profile and engine: a second Firefox of the same profile
/// cannot start (its profile is locked), and the session of the socket is the
/// one that keeps the page.
///
/// # Errors
/// See [`home`], or the path is longer than a Unix socket accepts.
pub(super) fn socket_path(profile: &str, engine: &str) -> Result<PathBuf> {
    let path = profile_root(profile)?.join(format!("host-{engine}.sock"));
    let length = path.as_os_str().len();
    if length > MAX_SOCKET_PATH_BYTES {
        return Err(Error::Config(format!(
            "the home of this machine is too long for the socket of a browser host: {} has {length} bytes, and one socket takes at most {MAX_SOCKET_PATH_BYTES}",
            path.display()
        )));
    }
    Ok(path)
}

/// The file in which the driver writes where its session stands, for a person
/// who looks at the machine of the host itself.
///
/// # Errors
/// See [`home`].
pub(super) fn state_path(profile: &str, engine: &str) -> Result<PathBuf> {
    Ok(profile_root(profile)?.join(format!("host-{engine}.json")))
}

/// The log of the session of this machine, which holds the words of a start
/// that failed before a link could read them.
///
/// # Errors
/// See [`home`].
pub(super) fn log_path(profile: &str, engine: &str) -> Result<PathBuf> {
    Ok(profile_root(profile)?.join(format!("host-{engine}.log")))
}

/// Create the profile folder of `profile` with the mode of a private folder.
///
/// # Errors
/// The folder cannot be created.
pub(super) fn ensure_profile_root(folder: &Path) -> Result<()> {
    std::fs::create_dir_all(folder)
        .map_err(|error| Error::Config(format!("could not create {}: {error}", folder.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| Error::Config(format!("could not protect {}: {error}", folder.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_SOCKET_PATH_BYTES, socket_path};

    #[test]
    fn the_socket_of_a_profile_names_its_engine_and_stays_inside_the_socket_bound() {
        let path = socket_path("mail", "firefox").expect("a socket path");
        assert!(
            path.ends_with("browser-profiles/mail/host-firefox.sock"),
            "{}",
            path.display()
        );
        let home = super::home().expect("a home of the test");
        assert!(
            path.starts_with(&home),
            "the socket is on this machine: {}",
            path.display()
        );
        assert!(
            path.as_os_str().len() <= MAX_SOCKET_PATH_BYTES,
            "a test home is short enough for one socket"
        );
    }
}
