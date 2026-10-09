//! A user token chain kept on this computer, readable only by its user: the permission to
//! publish images, and this computer's own sign-in for the connected app. Each is renewed
//! before use and stored again before its new token is used, since a renewal ends the old
//! chain.
use super::{Error, Result};
use horizon_cloud::github::{Chain, Client, Error as GitHubError, Secret};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

/// An access token with less time left is renewed first, so no use outlives it.
const MARGIN: Duration = Duration::from_mins(60);
/// The largest private file Horizon reads and rewrites here.
pub(super) const MAX_FILE: u64 = 1 << 20;

pub(super) struct Stored {
    pub login: String,
    pub access_token: Secret,
    pub access_expires_at: SystemTime,
    pub refresh_token: Secret,
    pub refresh_expires_at: SystemTime,
    /// The chain came from a web sign-in, whose renewal needs the app's client secret.
    pub web: bool,
}

/// Why a stored chain gave no token, when the person need not sign in again.
pub(super) enum Kept {
    /// GitHub could not be reached; the chain stays.
    Unreachable,
    /// GitHub refused without saying the chain is gone; it stays for the next try.
    Unconfirmed,
    Local(Error),
}

impl From<Error> for Kept {
    fn from(error: Error) -> Self {
        Self::Local(error)
    }
}

/// The account and a usable access token of the chain at `path`, renewing it when it is
/// close to its expiry, or `None` when the person must sign in: there is no chain, or
/// GitHub said it is gone, and then it is forgotten. `client_secret` reads the app's
/// secret, which only a chain from a web sign-in renews with; `None` for an app without one.
pub(super) fn current(
    path: &Path,
    client: &Client,
    client_id: &str,
    client_secret: Option<&dyn Fn() -> Result<Secret>>,
) -> std::result::Result<Option<(String, Secret)>, Kept> {
    let Some(stored) = load(path) else {
        return Ok(None);
    };
    let now = SystemTime::now();
    let token = if stored.access_expires_at > now + MARGIN {
        stored.access_token
    } else if stored.refresh_expires_at > now {
        // A secret that cannot be read is this computer's error, never GitHub's refusal.
        let secret = match (stored.web, client_secret) {
            (false, _) => None,
            (true, Some(read)) => Some(read()?),
            (true, None) => {
                return Err(Kept::Local(Error::Invalid(
                    "A web sign-in renews only with its app's secret",
                )));
            }
        };
        match client.refresh(client_id, secret.as_ref(), &stored.refresh_token) {
            // The old chain stopped working with this answer: store the new one first.
            Ok(chain) => {
                save(path, &stored.login, &chain, stored.web)?;
                chain.access_token
            }
            // Only GitHub's word that the chain is gone ends it.
            Err(GitHubError::Revoked) => return Ok(forget(path)?),
            Err(error) => return Err(kept(&error)),
        }
    } else {
        return Ok(forget(path)?);
    };
    // The person may have revoked the app on GitHub since; ask GitHub who the token is.
    match client.user(&token) {
        Ok(user) => Ok(Some((user.login, token))),
        Err(GitHubError::Revoked) => Ok(forget(path)?),
        Err(error) => Err(kept(&error)),
    }
}

fn kept(error: &GitHubError) -> Kept {
    if matches!(error, GitHubError::Transport) {
        Kept::Unreachable
    } else {
        Kept::Unconfirmed
    }
}

/// Drops a chain that no longer works, so the next use asks again.
pub(super) fn forget(path: &Path) -> Result<Option<(String, Secret)>> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn load(path: &Path) -> Option<Stored> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Fields {
        login: String,
        access_token: Secret,
        access_expires_at: u64,
        refresh_token: Secret,
        refresh_expires_at: u64,
        #[serde(default)]
        web: bool,
    }
    let bytes = Zeroizing::new(read_private(path).ok()??);
    let fields: Fields = serde_json::from_slice(&bytes).ok()?;
    let at = |seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds));
    (valid_login(&fields.login) && valid_token(&fields.access_token) && valid_token(&fields.refresh_token))
        .then_some(())?;
    Some(Stored {
        login: fields.login,
        access_token: fields.access_token,
        access_expires_at: at(fields.access_expires_at)?,
        refresh_token: fields.refresh_token,
        refresh_expires_at: at(fields.refresh_expires_at)?,
        web: fields.web,
    })
}

/// Stores `chain` for `login` at `path`, replacing what was there in one rename.
pub(super) fn save(path: &Path, login: &str, chain: &Chain, web: bool) -> Result<()> {
    /// Borrowed, so no copy of a token outlives the wiped buffer below.
    #[derive(Serialize)]
    struct Fields<'a> {
        login: &'a str,
        access_token: &'a str,
        access_expires_at: u64,
        refresh_token: &'a str,
        refresh_expires_at: u64,
        web: bool,
    }
    let seconds = |at: SystemTime| at.duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
    // Sized up front, so no reallocation frees an unwiped copy.
    let mut value = Zeroizing::new(Vec::with_capacity(8192));
    serde_json::to_writer(
        &mut *value,
        &Fields {
            login,
            access_token: chain.access_token.expose(),
            access_expires_at: seconds(chain.access_expires_at),
            refresh_token: chain.refresh_token.expose(),
            refresh_expires_at: seconds(chain.refresh_expires_at),
            web,
        },
    )
    .map_err(|_| Error::Json)?;
    let (Some(directory), Some(name)) = (path.parent(), path.file_name().and_then(|name| name.to_str())) else {
        return Err(Error::Invalid("A Horizon credential file needs a directory and a name"));
    };
    private_directory(directory)?;
    write_private(directory, name, &value)
}

pub(super) fn valid_login(login: &str) -> bool {
    (1..=39).contains(&login.len()) && login.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_token(token: &Secret) -> bool {
    let token = token.expose();
    (1..=2048).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_graphic())
}

/// A private file's bytes, or `None` when it does not exist. A file that others could
/// read or replace is refused.
pub(super) fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("A Horizon credential file must be private"));
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_FILE {
        return Err(Error::Invalid("A Horizon credential file is not a private file"));
    }
    // Sized for the whole read up front, so no reallocation frees an unwiped copy.
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0) + 1);
    file.take(MAX_FILE).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Replaces `directory/name` with `bytes` in one rename, readable only by this user.
pub(super) fn write_private(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::Builder::new()
        .prefix(&format!(".{name}-"))
        .tempfile_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file().set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(directory.join(name)).map_err(|error| error.error)?;
    Ok(())
}

/// Creates `directory` readable only by this user; an existing link is refused.
pub(super) fn private_directory(directory: &Path) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    if std::fs::symlink_metadata(directory)?.file_type().is_symlink() {
        return Err(Error::Invalid("A Horizon credential directory must not be a link"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Holds the lock file at `path` until the returned file is dropped, asking `wait` before
/// each retry while another process holds it.
pub(super) fn lock(path: &Path, mut wait: impl FnMut() -> Result<()>) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                wait()?;
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}
