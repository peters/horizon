//! Sign in with `ChatGPT` for Codex: one login this computer owns and gives to each cloud.
//!
//! The Codex CLI itself signs in, into a private Codex home beside the cloud settings, so
//! the person's own `~/.codex` login is never used or replaced. Each deployment, reconnect
//! and rebuild copies the login to the worker, unless the worker holds a later renewal of
//! the same account; that renewal then comes back here, because `ChatGPT` refresh tokens
//! are single use and the copy here would otherwise be spent.
mod renewal;

use super::{Error, Event, Result, command::Runner, ssh::Connection};
pub use renewal::{Delivered, Kept, Renewal, fingerprint, keep_cloud_current, renew, sync};
use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

/// The private Codex home of the login, under the cloud root.
const DIRECTORY: &str = "credentials/codex";
const FILE: &str = "auth.json";
/// Larger than any login Codex writes, and within a private worker exchange.
const MAX_FILE: u64 = 64 * 1024;
/// How long the browser sign-in may take before it is given up.
const SIGN_IN: Duration = Duration::from_mins(15);
const CODEX: &str = "codex";

/// The Codex home that holds the `ChatGPT` login of the cloud root `root`.
#[must_use]
pub fn home(root: &Path) -> PathBuf {
    root.join(DIRECTORY)
}

/// The signed-in account, as the settings show it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// The `ChatGPT` account ID, which tells two accounts apart.
    pub id: String,
    pub email: Option<String>,
    /// The `ChatGPT` plan, such as `plus` or `pro`.
    pub plan: Option<String>,
}

/// Why a sign-in did not finish.
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("Install the Codex CLI on this computer to sign in with ChatGPT")]
    NoCodex,
    #[error("{0}")]
    Codex(String),
    #[error(transparent)]
    Cloud(#[from] Error),
}

/// The account of the login in `home`, or `None` when there is no usable `ChatGPT` login.
/// # Errors
/// The login file is not a private file, or it could not be read.
pub fn account(home: &Path) -> Result<Option<Account>> {
    // A login that `OpenAI` refused for good needs a new sign-in, so it shows as none.
    Ok(read(home)?
        .filter(|login| !renewal::marked_refused(home, login))
        .map(|login| login.account))
}

/// Signs in with the system browser through `codex login` and returns the account. A
/// sign-in replaces the earlier login of `home`, which is revoked once the new login is in
/// place, so clouds holding it get the new one when they next connect. Blocks until the browser flow ends; run it
/// off the UI thread.
/// # Errors
/// Codex is missing, the person cancelled, or Codex reported that the sign-in failed.
pub fn sign_in(home: &Path, cancel: &super::Cancellation) -> std::result::Result<Account, SignInError> {
    sign_in_with(CODEX, home, cancel)
}

fn sign_in_with(program: &str, home: &Path, cancel: &super::Cancellation) -> std::result::Result<Account, SignInError> {
    private_directory(home)?;
    remove_abandoned_sign_ins(home);
    // Codex signs in into a private staging home, removed with it, so a cloud that
    // connects during the browser flow still gets the login here, and the new login
    // replaces it in one rename under the lock that clouds and sign-out take.
    let staging = tempfile::Builder::new()
        .prefix(".sign-in-")
        .tempdir_in(home)
        .map_err(Error::from)?;
    let mut command = codex(program, staging.path());
    // Codex keeps the login in the file, never the system keyring, so it can be copied.
    command.args(["login", "-c", "cli_auth_credentials_store=\"file\""]);
    run(&mut command, cancel, "ChatGPT sign-in", SIGN_IN)?;
    let login =
        read(staging.path())?.ok_or_else(|| SignInError::Codex("Codex finished without a ChatGPT login".into()))?;
    let _lock = lock(home)?;
    if cancel.is_cancelled() {
        return Err(SignInError::Codex("The ChatGPT sign-in was cancelled".into()));
    }
    // An earlier login, such as one another Horizon window made, is revoked once the new
    // one is in place, so a worker that still holds it cannot renew it into a later login
    // that replaces this one. A file that cannot be read is replaced without revocation.
    let earlier = read(home).ok().flatten();
    write_private(home, &login.bytes)?;
    if let Some(earlier) = earlier {
        revoke_copy(program, home, &earlier.bytes);
    }
    Ok(login.account)
}

/// Revokes a login that is no longer in `home`, from a private copy that Codex removes.
fn revoke_copy(program: &str, home: &Path, bytes: &[u8]) {
    let revoked = tempfile::Builder::new()
        .prefix(".sign-in-")
        .tempdir_in(home)
        .map_err(Error::from)
        .and_then(|copy| write_private(copy.path(), bytes).map(|()| copy));
    match revoked {
        Ok(copy) => revoke(program, copy.path()),
        Err(error) => tracing::warn!(%error, "could not revoke the earlier ChatGPT login"),
    }
}

/// Removes what Horizon left behind when it stopped during a sign-in or a write: staging
/// homes and unfinished login files, which may hold a login. One younger than a sign-in
/// may belong to another window. Best effort: what cannot be removed now is tried again
/// at the next sign-in or sign-out.
fn remove_abandoned_sign_ins(home: &Path) {
    let Ok(entries) = std::fs::read_dir(home) else {
        return;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with(".sign-in-") || name.starts_with(".auth-")) {
            continue;
        }
        let abandoned = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                modified
                    .elapsed()
                    .is_ok_and(|age| age > SIGN_IN + Duration::from_mins(5))
            });
        if !abandoned {
            continue;
        }
        let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            std::fs::remove_dir_all(entry.path())
        } else {
            std::fs::remove_file(entry.path())
        };
        if let Err(error) = removed
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "could not remove an abandoned ChatGPT sign-in");
        }
    }
}

/// Revokes the login of `home` at `OpenAI` with `codex logout`, which also removes it.
/// Revocation is a courtesy to the account; without Codex or `OpenAI` it is skipped.
fn revoke(program: &str, home: &Path) {
    let mut command = codex(program, home);
    command.arg("logout");
    if let Err(error) = run(
        &mut command,
        &super::Cancellation::default(),
        "ChatGPT sign-out",
        Duration::from_secs(30),
    ) {
        tracing::warn!(%error, "codex logout did not revoke the ChatGPT login");
    }
}

/// Signs out of `shown`, the account the person saw: `codex logout` revokes the login at
/// `OpenAI` when it can, and the login leaves this computer in any case. Clouds keep
/// working until their current access ends.
/// # Errors
/// Another Horizon window signed in to another account since, or the login file could
/// not be read or removed.
pub fn sign_out(home: &Path, shown: &Account) -> Result<()> {
    sign_out_with(CODEX, home, shown)
}

fn sign_out_with(program: &str, home: &Path, shown: &Account) -> Result<()> {
    if !home.is_dir() {
        return Ok(());
    }
    // A cloud connecting now cannot bring the login back after it is gone.
    let _lock = lock(home)?;
    remove_abandoned_sign_ins(home);
    // A login file that cannot be read, such as one with open permissions, goes too.
    match read(home) {
        Ok(None) if !home.join(FILE).exists() => return Ok(()),
        // A later renewal of the same account is the login shown; another account is not.
        Ok(Some(login)) if login.account_id != shown.id => {
            return Err(Error::Invalid(
                "Another Horizon window signed in to another ChatGPT account, shown here now. Nothing was signed out.",
            ));
        }
        Ok(Some(_)) => revoke(program, home),
        _ => {}
    }
    match std::fs::remove_file(home.join(FILE)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    // Durable, so a power loss now cannot bring the login back for a later cloud.
    sync_directory(home)
}

/// Gives the login in `home` to the worker behind `connection`, as its Codex login, and
/// keeps the worker's own login when it is a later renewal of the same account. That later
/// renewal replaces the one here. Without a `ChatGPT` login in `home`, nothing changes: the
/// worker keeps any login made in its own terminal.
/// # Errors
/// The local login could not be read or written, or the worker refused it.
pub(super) fn give(home: &Path, connection: &Connection, runner: &Runner<'_>) -> Result<()> {
    if !home.is_dir() {
        return Ok(());
    }
    exchange(home, &mut connection.command(&install_command()), runner)
}

/// Sends the login to `install`, the worker's install script, and keeps what it returns.
fn exchange(home: &Path, install: &mut Command, runner: &Runner<'_>) -> Result<()> {
    let _lock = lock(home)?;
    let Some(local) = read(home)? else {
        return Ok(());
    };
    // A login that OpenAI refused is not given to the worker; the worker only shows its own,
    // which may be a later renewal that makes the login here usable again.
    let given: &[u8] = if renewal::marked_refused(home, &local) {
        &[]
    } else {
        &local.bytes
    };
    let reply = runner.private_exchange(install, given, Duration::from_secs(30))?;
    adopt(home, &Zeroizing::new(reply))
}

/// Keeps `reply`, the login the worker holds now, when it renews the login here. The login
/// here is read again: a sign-in that finished meanwhile is newer and stays.
fn adopt(home: &Path, reply: &[u8]) -> Result<()> {
    // A worker asked only to show its login may have none.
    if reply.is_empty() {
        return Ok(());
    }
    let remote = Login::parse(reply).ok_or(Error::Invalid("The worker returned an invalid ChatGPT login"))?;
    if read(home)?.is_some_and(|current| remote.renews(&current)) {
        write_private(home, reply)?;
    }
    Ok(())
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

/// The script that installs a login from standard input as the Codex login of `home` and
/// prints the one it keeps.
fn install_script(home: &str) -> String {
    format!(
        "python3 -c {} {}",
        quote(include_str!("codex_login/install.py")),
        quote(home)
    )
}

/// The worker command that runs [`install_script`] as the agent user, for its home.
fn install_command() -> String {
    let command = install_script("/workspace/home");
    format!(
        "if [ -f /run/horizon-tailnet/agent-isolation ]; then exec horizon-worker-tailnet agent /bin/sh -c {}; else exec /bin/sh -c {}; fi",
        quote(&command),
        quote(&command),
    )
}

/// The fields of a stored login that Horizon reads. Tokens are base64url text without
/// escapes, so they borrow from the input; a login whose tokens do not is no login.
#[derive(serde::Deserialize)]
struct Stored<'a> {
    #[serde(borrow, default)]
    auth_mode: Option<&'a str>,
    #[serde(borrow, default)]
    tokens: Option<StoredTokens<'a>>,
    #[serde(borrow, default)]
    last_refresh: Option<&'a str>,
}

#[derive(serde::Deserialize)]
struct StoredTokens<'a> {
    #[serde(borrow, default)]
    id_token: Option<&'a str>,
    #[serde(borrow, default)]
    access_token: Option<&'a str>,
    #[serde(borrow, default)]
    refresh_token: Option<&'a str>,
    #[serde(borrow, default)]
    account_id: Option<&'a str>,
}

/// A `ChatGPT` login as Codex stores it: its bytes and what decides which one is newer.
struct Login {
    bytes: Zeroizing<Vec<u8>>,
    account_id: String,
    refreshed: Option<OffsetDateTime>,
    account: Account,
    refresh_token: Zeroizing<String>,
    /// When the access token was issued and when it ends, if its claims say so.
    access: Option<(OffsetDateTime, OffsetDateTime)>,
}

impl Login {
    fn parse(bytes: &[u8]) -> Option<Self> {
        // Borrowed from `bytes`, which is wiped, so no unwiped copy of a token is made.
        let stored: Stored<'_> = serde_json::from_slice(bytes).ok()?;
        if stored.auth_mode? != "chatgpt" {
            return None;
        }
        let tokens = stored.tokens?;
        let access = present(tokens.access_token)?;
        let refresh_token = Zeroizing::new(present(tokens.refresh_token)?.to_owned());
        let account_id = present(tokens.account_id)?.to_owned();
        let refreshed = stored
            .last_refresh
            .and_then(|text| OffsetDateTime::parse(text, &Rfc3339).ok());
        Some(Self {
            account: Account {
                id: account_id.clone(),
                ..present(tokens.id_token).map(claims).unwrap_or_default()
            },
            access: lifetime(access),
            bytes: Zeroizing::new(bytes.to_vec()),
            account_id,
            refreshed,
            refresh_token,
        })
    }

    fn access_lifetime(&self) -> Option<(OffsetDateTime, OffsetDateTime)> {
        self.access
    }

    fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    /// Whether this login is a later renewal of `other`'s account.
    fn renews(&self, other: &Self) -> bool {
        self.account_id == other.account_id
            && match (self.refreshed, other.refreshed) {
                (Some(this), Some(that)) => this > that,
                (Some(_), None) => true,
                (None, _) => false,
            }
    }
}

/// `value` when it holds text.
fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

/// The claims of a token, unverified: Horizon reads them only to show the account and to
/// time renewals, as Codex does.
fn payload(token: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    token
        .split('.')
        .nth(1)
        .and_then(|part| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part).ok())
        .map(Zeroizing::new)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
}

/// When an access token was issued and when it ends.
fn lifetime(token: &str) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let payload = payload(token)?;
    let time = |name: &str| OffsetDateTime::from_unix_timestamp(payload.get(name)?.as_i64()?).ok();
    let (issued, expires) = (time("iat")?, time("exp")?);
    (issued < expires).then_some((issued, expires))
}

/// The account an ID token names, for display only. Codex checked the token when it
/// signed in; nothing here relies on these claims for access.
fn claims(token: &str) -> Account {
    let Some(payload) = payload(token) else {
        return Account::default();
    };
    let text = |value: Option<&serde_json::Value>| value.and_then(serde_json::Value::as_str).map(str::to_owned);
    Account {
        id: String::new(),
        email: text(payload.get("email")),
        plan: text(payload.pointer("/https:~1~1api.openai.com~1auth/chatgpt_plan_type")),
    }
}

fn read(home: &Path) -> Result<Option<Login>> {
    let path = home.join(FILE);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("The ChatGPT login file must be private (0600)"));
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_FILE {
        return Err(Error::Invalid("The ChatGPT login is not a private file"));
    }
    // Sized for the whole read up front, so no reallocation frees an unwiped copy.
    let mut bytes = Zeroizing::new(Vec::with_capacity(usize::try_from(MAX_FILE).unwrap_or(0) + 1));
    file.take(MAX_FILE).read_to_end(&mut bytes)?;
    // Codex rewrites the file in place, so a sign-in finishing now can leave it half
    // written for a moment; that reads as no login rather than an error.
    Ok(Login::parse(&bytes))
}

/// Replaces the login in one rename, readable only by this user.
fn write_private(home: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::Builder::new().prefix(".auth-").tempfile_in(home)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file().set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(home.join(FILE)).map_err(|error| error.error)?;
    sync_directory(home)
}

/// Makes a rename or removal in `home` durable.
fn sync_directory(home: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(home)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = home;
    Ok(())
}

/// Creates `home` readable only by this user; an existing link is refused.
fn private_directory(home: &Path) -> Result<()> {
    std::fs::create_dir_all(home)?;
    if std::fs::symlink_metadata(home)?.file_type().is_symlink() {
        return Err(Error::Invalid("The ChatGPT login directory must not be a link"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Holds the login while one cloud exchanges it, so two clouds connecting at once never
/// both spend the same renewal.
fn lock(home: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join(".horizon.lock"))?;
    file.lock()?;
    Ok(file)
}

fn codex(program: &str, home: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env("CODEX_HOME", home)
        // A key or access token in Horizon's environment must not decide the login.
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("CODEX_ACCESS_TOKEN")
        .stdin(std::process::Stdio::null());
    command
}

/// Runs a Codex login command and turns its failure into the message Codex printed.
fn run(
    command: &mut Command,
    cancel: &super::Cancellation,
    name: &'static str,
    timeout: Duration,
) -> std::result::Result<(), SignInError> {
    let reported = RefCell::new(None);
    // Codex prints the sign-in address and progress; only its error line is kept.
    let emit = |event: Event| {
        if let Event::Output(output) = event
            && let Some(line) = output.lines().map(str::trim).rfind(|line| line.starts_with("Error"))
        {
            *reported.borrow_mut() = Some(format!("Codex: {line}"));
        }
    };
    let runner = Runner {
        cancel,
        emit: &emit,
        secrets: Vec::new(),
    };
    match runner.run(name, command, timeout) {
        Ok(_) => Ok(()),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Err(SignInError::NoCodex),
        Err(Error::Command(_)) => Err(SignInError::Codex(
            reported
                .into_inner()
                .unwrap_or_else(|| format!("{name} did not finish. Try again.")),
        )),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
