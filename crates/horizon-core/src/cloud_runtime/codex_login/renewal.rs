//! Renews this computer's `ChatGPT` login before its access token ends, and gives the
//! renewal to the ready clouds, so no worker ever spends the shared refresh token itself.
//!
//! Codex on a worker renews its login five minutes before the access token ends, but it
//! first reads its login file again and takes a newer one it finds there instead. Horizon
//! renews well before that point and replaces the worker's file, so the workers keep using
//! the login this computer renewed. Renewal uses the same request as Codex.
use super::{Login, home, lock, read, sync_directory, write_private};
use crate::cloud_runtime::{
    Cancellation, Error, Result,
    command::Runner,
    settings::Settings,
    ssh::Connection,
    state::{Store, cloud_directory},
};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use zeroize::Zeroizing;

/// The token endpoint and client of the Codex CLI's `ChatGPT` sign-in.
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Renewal starts when less than this is left of the access token, or less than half
/// of its life, whichever comes first.
const AHEAD: Duration = Duration::from_hours(48);
/// Without a readable expiry, a login this old is renewed, as Codex does.
const STALE: Duration = Duration::from_hours(7 * 24);
const MAX_REPLY: u64 = 64 * 1024;
/// A cloud that has the current login is asked again after this long, so a renewal its
/// worker made on its own comes back here soon.
const RESEND: Duration = Duration::from_mins(30);

/// Per cloud, the login fingerprint its worker last got and when.
pub type Delivered = HashMap<String, (String, Instant)>;

/// What a renewal did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renewal {
    /// There is no `ChatGPT` login on this computer.
    NoLogin,
    /// The access token has enough time left.
    NotDue,
    Renewed,
    /// `OpenAI` refused the refresh token for good, for example because a worker spent it
    /// while this computer was away; the login was removed and needs a new sign-in.
    SignInAgain,
}

/// What [`keep_cloud_current`] did for one cloud.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The worker now holds the login of this computer, or a later renewal that this
    /// computer took over.
    Current,
    /// Nothing to do: no login here, the cloud is not ready, it runs no Codex, it uses an
    /// API key, or another operation holds it.
    Skipped,
}

/// Renews the login in the cloud root `root` when its access token is due.
/// # Errors
/// The login could not be read or written, or `OpenAI` could not be reached or answered
/// with a temporary failure; the login then stays as it is.
pub fn renew(root: &Path, cancel: &Cancellation) -> Result<Renewal> {
    renew_with(&home(root), TOKEN_URL, OffsetDateTime::now_utc(), cancel)
}

pub(super) fn renew_with(home: &Path, url: &str, now: OffsetDateTime, cancel: &Cancellation) -> Result<Renewal> {
    if !home.is_dir() {
        return Ok(Renewal::NoLogin);
    }
    let _lock = lock(home)?;
    let Some(login) = read(home)? else {
        return Ok(Renewal::NoLogin);
    };
    if marked_refused(home, &login) {
        return Ok(Renewal::SignInAgain);
    }
    if !due(&login, now) {
        return Ok(Renewal::NotDue);
    }
    cancel.check()?;
    match request(url, &login)? {
        Answer::Tokens(answer) => {
            write_private(home, &renewed(&login.bytes, &answer, now)?)?;
            Ok(Renewal::Renewed)
        }
        Answer::Refused => {
            // The refresh token can never work again. The login stays, marked: a worker may
            // still hold a later renewal of it, which the next sync takes over.
            write_marker(home, &identity(&login))?;
            Ok(Renewal::SignInAgain)
        }
    }
}

/// Gives the login of this computer to the worker of `cloud` when it is ready, runs
/// Codex without an API key, and no other operation holds it, and takes over a later
/// renewal the worker holds. A deployment does the same as one of its steps.
/// # Errors
/// The settings, the cloud record or the login could not be read, or the worker could
/// not be reached or refused the login.
pub fn keep_cloud_current(root: &Path, cloud: &str, cancel: &Cancellation) -> Result<Kept> {
    let home = home(root);
    if read(&home)?.is_none() {
        return Ok(Kept::Skipped);
    }
    let settings = Settings::load(&root.join("settings.json"))?;
    if settings.openai_api_key_file.is_some() {
        return Ok(Kept::Skipped);
    }
    let directory = cloud_directory(root, cloud)?;
    // Held only to read the record: a deployment, stop or resize under way is not delayed,
    // and the pinned host key binds the exchange to the recorded worker.
    let worker = match Store::lock_existing(&directory) {
        Ok(store) => store.load()?.and_then(|state| {
            (state.cloud_id == cloud && state.worker_ready() && state.profile.capabilities.permits_agent("codex"))
                .then_some(state.worker)
                .flatten()
        }),
        Err(Error::Busy) => None,
        Err(error) => return Err(error),
    };
    let Some(worker) = worker else {
        return Ok(Kept::Skipped);
    };
    let connection = Connection::new(&worker, &settings, &directory)?;
    let runner = Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    };
    // Never trust a new host key from a background job.
    super::exchange(
        &home,
        &mut connection.pinned_command(&super::install_command()),
        &runner,
    )?;
    Ok(Kept::Current)
}

/// Keeps the login current everywhere: first takes over any renewal a worker made on its
/// own, then renews the login here when it is due, then gives the renewal to every cloud.
/// Every cloud under `root` counts, also those of other Horizon sessions, so none is left
/// with a refresh token this renewal spent. A cloud is asked again when the login changed
/// since it last got it, or after [`RESEND`].
/// # Errors
/// The cloud folders could not be listed. A renewal or a cloud that fails is logged and
/// tried again at the next sync.
pub fn sync(root: &Path, delivered: &mut Delivered, cancel: &Cancellation) -> Result<Renewal> {
    let clouds = clouds(root)?;
    delivered.retain(|cloud, _| clouds.contains(cloud));
    // Before a renewal every cloud is asked, also one asked recently: a worker that renewed
    // on its own since holds the only refresh token that still works.
    let renewing = read(&home(root))
        .ok()
        .flatten()
        .is_some_and(|login| due(&login, OffsetDateTime::now_utc()));
    deliver(root, &clouds, delivered, renewing, cancel);
    let renewal = match renew(root, cancel) {
        Ok(renewal) => renewal,
        Err(error) => {
            tracing::warn!(%error, "could not renew the ChatGPT login for Codex");
            return Ok(Renewal::NotDue);
        }
    };
    if renewal == Renewal::Renewed {
        deliver(root, &clouds, delivered, false, cancel);
    }
    Ok(renewal)
}

/// Asks the clouds that do not have the login now here, or were asked longer ago than
/// [`RESEND`]; every cloud when `all`.
fn deliver(root: &Path, clouds: &[String], delivered: &mut Delivered, all: bool, cancel: &Cancellation) {
    for cloud in clouds {
        if cancel.is_cancelled() {
            return;
        }
        let Ok(Some(current)) = fingerprint(root) else {
            return;
        };
        if !all && current_on(delivered, cloud, &current) {
            continue;
        }
        match keep_cloud_current(root, cloud, cancel) {
            // The exchange may have taken over the worker's renewal; record what it holds.
            Ok(Kept::Current) => {
                if let Ok(Some(now)) = fingerprint(root) {
                    delivered.insert(cloud.clone(), (now, Instant::now()));
                }
            }
            Ok(Kept::Skipped) => {}
            Err(error) => tracing::warn!(%error, cloud, "could not give the ChatGPT login to a cloud"),
        }
    }
}

/// Whether `cloud` got the login `current` less than [`RESEND`] ago.
fn current_on(delivered: &Delivered, cloud: &str, current: &str) -> bool {
    delivered
        .get(cloud)
        .is_some_and(|(given, at)| given == current && at.elapsed() < RESEND)
}

/// The marker of a login whose refresh token `OpenAI` refused for good.
const REFUSED: &str = ".refused";

/// What tells one login from another: its account and when it was renewed.
fn identity(login: &Login) -> String {
    format!(
        "{}@{}",
        login.account_id,
        login.refreshed.map_or(0, OffsetDateTime::unix_timestamp_nanos)
    )
}

/// Whether `OpenAI` refused this very login; a later renewal or sign-in is a new login.
pub(super) fn marked_refused(home: &Path, login: &Login) -> bool {
    std::fs::read_to_string(home.join(REFUSED)).is_ok_and(|marked| marked == identity(login))
}

fn write_marker(home: &Path, identity: &str) -> Result<()> {
    let mut file = tempfile::Builder::new().prefix(".refused-").tempfile_in(home)?;
    std::io::Write::write_all(&mut file, identity.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(home.join(REFUSED)).map_err(|error| error.error)?;
    sync_directory(home)
}

/// The clouds with a record under `root`.
fn clouds(root: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut clouds: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().join("deployment.json").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| horizon_cloud::valid_id(name))
        .collect();
    clouds.sort();
    Ok(clouds)
}

/// What identifies the login now on this computer, so a cloud that already has it is not
/// asked again; `None` without a login.
/// # Errors
/// The login file could not be read.
pub fn fingerprint(root: &Path) -> Result<Option<String>> {
    Ok(read(&home(root))?.map(|login| identity(&login)))
}

/// Whether the access token of `login` is close enough to its end to renew it now.
fn due(login: &Login, now: OffsetDateTime) -> bool {
    let Some((issued, expires)) = login.access_lifetime() else {
        return login.refreshed.is_none_or(|refreshed| now - refreshed >= STALE);
    };
    let left = expires - now;
    let half = (expires - issued) / 2;
    left <= time::Duration::try_from(AHEAD).unwrap_or(time::Duration::MAX).min(half)
}

/// The new tokens of a renewal; each may be missing, as Codex allows. Borrowed from the
/// wiped answer, so no unwiped copy of a token is made.
#[derive(serde::Deserialize)]
struct Tokens<'a> {
    #[serde(borrow, default, rename = "id_token")]
    id: Option<&'a str>,
    #[serde(borrow, default, rename = "access_token")]
    access: Option<&'a str>,
    #[serde(borrow, default, rename = "refresh_token")]
    refresh: Option<&'a str>,
}

/// The renewal request, borrowed from the login.
#[derive(serde::Serialize)]
struct Request<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    refresh_token: &'a str,
}

enum Answer {
    /// The answer that holds the new tokens.
    Tokens(Zeroizing<Vec<u8>>),
    /// The refresh token was refused for good: spent, expired or revoked.
    Refused,
}

fn request(url: &str, login: &Login) -> Result<Answer> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    // Sized up front, so no reallocation frees an unwiped copy of the refresh token.
    let mut body = Zeroizing::new(Vec::with_capacity(login.refresh_token().len() + 256));
    serde_json::to_writer(
        &mut *body,
        &Request {
            client_id: CLIENT_ID,
            grant_type: "refresh_token",
            refresh_token: login.refresh_token(),
        },
    )
    .map_err(|_| Error::Json)?;
    let mut answer = agent
        .post(url)
        .header("Content-Type", "application/json")
        .send(&body[..])
        .map_err(|_| Error::Invalid("OpenAI could not be reached to renew the ChatGPT login"))?;
    let status = answer.status().as_u16();
    // Sized for the whole answer up front, so no reallocation frees an unwiped copy of the
    // new tokens.
    let mut bytes = Zeroizing::new(Vec::with_capacity(usize::try_from(MAX_REPLY).unwrap_or(0) + 1));
    std::io::Read::read_to_end(
        &mut std::io::Read::take(answer.body_mut().as_reader(), MAX_REPLY + 1),
        &mut bytes,
    )
    .map_err(|_| Error::Invalid("OpenAI sent an unreadable answer to the ChatGPT renewal"))?;
    if bytes.len() as u64 > MAX_REPLY {
        return Err(Error::Invalid("OpenAI sent too large an answer to the ChatGPT renewal"));
    }
    match status {
        200 => {
            let tokens: Tokens<'_> = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
            if tokens.access.is_none_or(str::is_empty) {
                return Err(Error::Invalid(
                    "OpenAI renewed the ChatGPT login without an access token",
                ));
            }
            Ok(Answer::Tokens(bytes))
        }
        // Only an answer that says so is permanent: the refresh token is spent, expired or
        // revoked. Anything else, such as a proxy's bare 401, is tried again later.
        400 | 401 if refused(&bytes) => Ok(Answer::Refused),
        _ => Err(Error::Invalid(
            "OpenAI could not renew the ChatGPT login now; Horizon tries again later",
        )),
    }
}

/// Whether an error answer says the refresh token can never work again.
fn refused(bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    let code = value
        .get("error")
        .and_then(|error| error.get("code").or(Some(error)))
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("code").and_then(serde_json::Value::as_str));
    matches!(
        code,
        Some("invalid_grant" | "refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated")
    )
}

/// The login `bytes` with the new tokens and renewal time, every other field kept.
/// The login `bytes` with the new tokens of `answer` and the renewal time, every other
/// field kept. The parsed login is wiped before it is dropped.
fn renewed(bytes: &[u8], answer: &[u8], now: OffsetDateTime) -> Result<Zeroizing<Vec<u8>>> {
    let tokens: Tokens<'_> = serde_json::from_slice(answer).map_err(|_| Error::Json)?;
    let mut value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| Error::Json)?;
    let written = fill(&mut value, &tokens, now, bytes.len());
    wipe(&mut value);
    written
}

/// Writes the new tokens and time into `value`, a stored login of `size` bytes.
fn fill(
    value: &mut serde_json::Value,
    tokens: &Tokens<'_>,
    now: OffsetDateTime,
    size: usize,
) -> Result<Zeroizing<Vec<u8>>> {
    let stored = value
        .get_mut("tokens")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or(Error::Json)?;
    let mut added = 0;
    for (name, token) in [
        ("id_token", tokens.id),
        ("access_token", tokens.access),
        ("refresh_token", tokens.refresh),
    ] {
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            added += token.len();
            if let Some(mut old) = stored.insert(name.into(), serde_json::Value::String(token.to_owned())) {
                wipe(&mut old);
            }
        }
    }
    let stamp = now
        .format(&Rfc3339)
        .map_err(|_| Error::Invalid("Invalid renewal time"))?;
    value["last_refresh"] = serde_json::Value::String(stamp);
    // Sized up front, so no reallocation frees an unwiped copy.
    let mut bytes = Zeroizing::new(Vec::with_capacity(size + added + 1024));
    serde_json::to_writer(&mut *bytes, value).map_err(|_| Error::Json)?;
    Ok(bytes)
}

/// Wipes every string in `value`, where the tokens of a login are.
fn wipe(value: &mut serde_json::Value) {
    use zeroize::Zeroize as _;
    match value {
        serde_json::Value::String(text) => text.zeroize(),
        serde_json::Value::Array(items) => items.iter_mut().for_each(wipe),
        serde_json::Value::Object(fields) => fields.values_mut().for_each(wipe),
        _ => {}
    }
}

#[cfg(test)]
mod tests;
