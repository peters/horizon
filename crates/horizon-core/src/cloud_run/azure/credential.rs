//! Azure identity through the official Azure CLI: the token stays in memory, is never
//! persisted or logged, and is refreshed by asking the CLI again. No OAuth is hand-rolled.
use super::{AzureError, MANAGEMENT_ENDPOINT, valid_subscription_id};
#[cfg(not(windows))]
use std::process::Child;
use std::{
    fmt,
    io::Read as _,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

const CLI_TIMEOUT: Duration = Duration::from_secs(60);
const CLI_OUTPUT_LIMIT: usize = 64 * 1024;
const REFRESH_MARGIN: Duration = Duration::from_secs(300);
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const READER_GRACE: Duration = Duration::from_secs(2);
#[cfg(windows)]
const DEFAULT_EXECUTABLE: &str = "az.cmd";
#[cfg(not(windows))]
const DEFAULT_EXECUTABLE: &str = "az";

/// A bearer token for Azure Resource Manager and the instant it stops being usable.
#[derive(Clone)]
pub struct AzureAccessToken {
    secret: String,
    expires_at: Instant,
}

impl AzureAccessToken {
    /// # Errors
    /// Rejects empty, oversized or non-printable tokens and an expiry beyond the clock range.
    pub fn new(secret: impl Into<String>, valid_for: Duration) -> Result<Self, AzureError> {
        let secret = secret.into();
        let valid =
            !secret.is_empty() && secret.len() <= MAX_TOKEN_BYTES && secret.bytes().all(|b| b.is_ascii_graphic());
        if !valid {
            return Err(AzureError::CredentialUnavailable {
                reason: "token has an invalid shape",
            });
        }
        let expires_at = Instant::now()
            .checked_add(valid_for)
            .ok_or(AzureError::CredentialUnavailable {
                reason: "token expiry is out of range",
            })?;
        Ok(Self { secret, expires_at })
    }

    #[must_use]
    pub fn needs_refresh(&self) -> bool {
        Instant::now() + REFRESH_MARGIN >= self.expires_at
    }

    /// The `Authorization` header value; the only way the secret leaves this type.
    #[must_use]
    pub fn authorization_header(&self) -> String {
        format!("Bearer {}", self.secret)
    }
}

impl fmt::Debug for AzureAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AzureAccessToken(<redacted>)")
    }
}

/// Source of Resource Manager tokens for one explicit subscription.
pub trait AzureCredentialSource: Send + Sync {
    /// # Errors
    /// Returns a redacted error when no usable token can be obtained.
    fn token(&self) -> Result<AzureAccessToken, AzureError>;

    /// A token obtained within `budget`, for callers polling under an absolute
    /// deadline. The default serves sources that answer at once (a cached or synthetic
    /// token); a source that may block, as the CLI does on a refresh, must bound its
    /// own waiting by the budget and answer [`AzureError::CredentialUnavailable`] once
    /// it is spent.
    /// # Errors
    /// Returns a redacted error when no usable token can be obtained in time.
    fn token_within(&self, budget: Duration) -> Result<AzureAccessToken, AzureError> {
        let _ = budget;
        self.token()
    }
}

impl<F> AzureCredentialSource for F
where
    F: Fn() -> Result<AzureAccessToken, AzureError> + Send + Sync,
{
    fn token(&self) -> Result<AzureAccessToken, AzureError> {
        self()
    }
}

/// Tokens from `az account get-access-token` (CLI 2.54 or newer) for the configured
/// subscription: argument array, no shell, time and output bounds, explicit
/// subscription so the operator's default is never used, in-memory cache until expiry.
pub struct AzureCliCredential {
    executable: PathBuf,
    subscription_id: String,
    timeout: Duration,
    output_limit: usize,
    cached: Mutex<Option<AzureAccessToken>>,
}

impl AzureCliCredential {
    /// # Errors
    /// Rejects a malformed subscription identifier.
    pub fn new(subscription_id: impl Into<String>) -> Result<Self, AzureError> {
        Self::with_executable(PathBuf::from(DEFAULT_EXECUTABLE), subscription_id)
    }

    /// # Errors
    /// Rejects a malformed subscription identifier.
    pub fn with_executable(executable: PathBuf, subscription_id: impl Into<String>) -> Result<Self, AzureError> {
        let subscription_id = subscription_id.into();
        if !valid_subscription_id(&subscription_id) {
            return Err(AzureError::InvalidProfile);
        }
        Ok(Self {
            executable,
            subscription_id,
            timeout: CLI_TIMEOUT,
            output_limit: CLI_OUTPUT_LIMIT,
            cached: Mutex::new(None),
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn with_bounds(mut self, timeout: Duration, output_limit: usize) -> Self {
        self.timeout = timeout;
        self.output_limit = output_limit;
        self
    }

    /// Run the CLI once, ending the whole tree if it is not done by `deadline`.
    fn fetch(&self, deadline: Instant) -> Result<AzureAccessToken, AzureError> {
        let unavailable = |reason| AzureError::CredentialUnavailable { reason };
        let left = || deadline.saturating_duration_since(Instant::now());
        let mut command = Command::new(&self.executable);
        command
            .args([
                "account",
                "get-access-token",
                "--resource",
                &format!("{MANAGEMENT_ENDPOINT}/"),
                "--subscription",
                &self.subscription_id,
                "--output",
                "json",
                "--only-show-errors",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // The CLI is a launcher; its own process group lets a timeout end the whole tree.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        let mut child = OwnedChild(spawn_tree(command).map_err(|_| unavailable("Azure CLI could not be started"))?);
        // From here on every exit hands the tree to the detached reaper: no path after
        // the spawn waits on process teardown from the caller's thread.
        let Some(stdout) = child.take_stdout() else {
            reap(child, None);
            return Err(unavailable("Azure CLI produced no output"));
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let limit = self.output_limit as u64 + 1;
        let reader = std::thread::Builder::new()
            .name("azure-cli-output".into())
            .spawn(move || {
                let mut output = Vec::new();
                let result = stdout.take(limit).read_to_end(&mut output).map(|_| output);
                let _ = sender.send(result);
            });
        if reader.is_err() {
            reap(child, Some(receiver));
            return Err(unavailable("Azure CLI output could not be read"));
        }
        let status = loop {
            // Deadline first: a process that finished while the last sleep crossed it
            // is still late, and the sleep itself never overshoots the bound.
            let left = left();
            if left.is_zero() {
                return Err(abandon(child, receiver));
            }
            match child.0.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(20).min(left)),
                Err(_) => {
                    reap(child, Some(receiver));
                    return Err(unavailable("Azure CLI could not be waited on"));
                }
            }
        };
        // A descendant that inherited stdout can outlive the leader; the same bound applies.
        let output = match receiver.recv_timeout(left()) {
            Ok(Ok(output)) => output,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                reap(child, None);
                return Err(unavailable("Azure CLI output could not be read"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(abandon(child, receiver)),
        };
        // The leader has exited and its output is in hand; ending the rest of the
        // tree is bookkeeping that must not sit on the caller's deadline either.
        reap(child, None);
        if !status.success() || output.len() > self.output_limit {
            return Err(unavailable("Azure CLI did not return a token"));
        }
        let token = parse_cli_token(&output)?;
        // Output that arrived right at the bound is still late for the caller.
        if Instant::now() > deadline {
            return Err(unavailable("Azure CLI timed out"));
        }
        Ok(token)
    }
}

impl AzureCliCredential {
    /// Serve from the cache or refresh, with the cache lock wait and the CLI run both
    /// bounded by `budget` (a refresh in another thread holds the lock for its whole run).
    fn token_bounded(&self, budget: Duration) -> Result<AzureAccessToken, AzureError> {
        let exceeded = || AzureError::CredentialUnavailable {
            reason: "Azure CLI refresh in progress exceeded the caller's budget",
        };
        // The public budget is clamped to the credential's own timeout before it gets
        // here, so this cannot overflow; a redacted error, never a panic, if it ever did.
        let Some(deadline) = Instant::now().checked_add(budget) else {
            return Err(AzureError::CredentialUnavailable {
                reason: "token budget is out of range",
            });
        };
        let mut cached = loop {
            match self.cached.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(exceeded());
                    }
                    std::thread::sleep(Duration::from_millis(10).min(left));
                }
            }
        };
        // The lock may have been won only after the budget ran out (a refresh that
        // finished late): a late answer is not an answer, cached or not, and no refresh
        // starts past the deadline. Abandoning a run is detached, so an in-budget run
        // may use everything that is left.
        if Instant::now() >= deadline {
            return Err(exceeded());
        }
        if let Some(token) = cached.as_ref().filter(|token| !token.needs_refresh()) {
            return Ok(token.clone());
        }
        // The same absolute deadline governs the run, so nothing is re-measured.
        let token = self.fetch(deadline)?;
        *cached = Some(token.clone());
        Ok(token)
    }
}

impl AzureCredentialSource for AzureCliCredential {
    fn token(&self) -> Result<AzureAccessToken, AzureError> {
        let mut cached = self.cached.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(token) = cached.as_ref().filter(|token| !token.needs_refresh()) {
            return Ok(token.clone());
        }
        let token = self.fetch(Instant::now() + self.timeout)?;
        *cached = Some(token.clone());
        Ok(token)
    }

    fn token_within(&self, budget: Duration) -> Result<AzureAccessToken, AzureError> {
        self.token_bounded(budget.min(self.timeout))
    }
}

/// Only the executable's file name is shown; a configured path could carry a user name.
impl fmt::Debug for AzureCliCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AzureCliCredential")
            .field("executable", &self.executable.file_name().unwrap_or_default())
            .finish_non_exhaustive()
    }
}

/// End a CLI tree off the caller's thread: the kill, the wait and (when a reader is
/// still attached) the reader grace run on a detached thread, so the caller's deadline
/// never waits on process teardown. Killing every member closes the pipe writers, so
/// the reader returns on its own within the grace period. If no thread can be
/// spawned, the closure is dropped here together with the child, whose drop ends the
/// tree synchronously: slower, never leaked.
fn reap(child: OwnedChild, receiver: Option<mpsc::Receiver<std::io::Result<Vec<u8>>>>) {
    let reaper = std::thread::Builder::new().name("azure-cli-reaper".into());
    let spawned = reaper.spawn(move || {
        drop(child);
        if let Some(receiver) = receiver {
            let _ = receiver.recv_timeout(READER_GRACE);
        }
    });
    drop(spawned);
}

/// Give up on a tree that exceeded its bound without waiting for it to die.
fn abandon(child: OwnedChild, receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>) -> AzureError {
    reap(child, Some(receiver));
    AzureError::CredentialUnavailable {
        reason: "Azure CLI timed out",
    }
}

/// The CLI leader plus every descendant: a process group on Unix (the group id stays
/// valid after the leader exits) and a Job Object on Windows.
#[cfg(windows)]
type CliChild = Box<dyn process_wrap::std::ChildWrapper>;
#[cfg(not(windows))]
type CliChild = Child;

fn spawn_tree(command: Command) -> std::io::Result<CliChild> {
    #[cfg(windows)]
    {
        let mut command = process_wrap::std::CommandWrap::from(command);
        command.wrap(process_wrap::std::JobObject);
        command.spawn()
    }
    #[cfg(not(windows))]
    {
        let mut command = command;
        command.spawn()
    }
}

/// Ends and reaps the whole CLI process tree when the caller gives up on it, whether or
/// not the leader has already exited.
struct OwnedChild(CliChild);

impl OwnedChild {
    fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        #[cfg(windows)]
        {
            self.0.stdout().take()
        }
        #[cfg(not(windows))]
        {
            self.0.stdout.take()
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // `--` keeps the negative (process group) id from being read as an option.
        #[cfg(unix)]
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", self.0.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // On Windows this terminates the Job Object, which covers descendants.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Parse the CLI's JSON: `accessToken` plus `expires_on` (POSIX seconds, the field the
/// CLI documents as authoritative). A missing or past expiry is rejected so a broken
/// CLI cannot turn every call into a fresh process spawn.
pub(crate) fn parse_cli_token(output: &[u8]) -> Result<AzureAccessToken, AzureError> {
    let unavailable = |reason| AzureError::CredentialUnavailable { reason };
    let value: serde_json::Value =
        serde_json::from_slice(output).map_err(|_| unavailable("Azure CLI token response was malformed"))?;
    let secret = value
        .get("accessToken")
        .and_then(serde_json::Value::as_str)
        .ok_or(unavailable("Azure CLI token response was malformed"))?;
    let expires_on = value
        .get("expires_on")
        .and_then(|raw| raw.as_u64().or_else(|| raw.as_str().and_then(|text| text.parse().ok())))
        .ok_or(unavailable("Azure CLI token response lacked an expiry"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let remaining = expires_on
        .checked_sub(now)
        .filter(|remaining| *remaining > REFRESH_MARGIN.as_secs())
        .ok_or(unavailable("Azure CLI returned an expired token"))?;
    AzureAccessToken::new(secret, Duration::from_secs(remaining))
}
