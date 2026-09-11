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

    fn fetch(&self) -> Result<AzureAccessToken, AzureError> {
        let unavailable = |reason| AzureError::CredentialUnavailable { reason };
        let started = Instant::now();
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
        let stdout = child.take_stdout().ok_or(unavailable("Azure CLI produced no output"))?;
        let (sender, receiver) = mpsc::sync_channel(1);
        let limit = self.output_limit as u64 + 1;
        std::thread::Builder::new()
            .name("azure-cli-output".into())
            .spawn(move || {
                let mut output = Vec::new();
                let result = stdout.take(limit).read_to_end(&mut output).map(|_| output);
                let _ = sender.send(result);
            })
            .map_err(|_| unavailable("Azure CLI output could not be read"))?;
        let status = loop {
            if let Some(status) = child
                .0
                .try_wait()
                .map_err(|_| unavailable("Azure CLI could not be waited on"))?
            {
                break status;
            }
            if started.elapsed() >= self.timeout {
                return Err(abandon(child, &receiver));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // A descendant that inherited stdout can outlive the leader; the same bound applies.
        let output = match receiver.recv_timeout(self.timeout.saturating_sub(started.elapsed())) {
            Ok(Ok(output)) => output,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(unavailable("Azure CLI output could not be read"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(abandon(child, &receiver)),
        };
        if !status.success() || output.len() > self.output_limit {
            return Err(unavailable("Azure CLI did not return a token"));
        }
        parse_cli_token(&output)
    }
}

impl AzureCredentialSource for AzureCliCredential {
    fn token(&self) -> Result<AzureAccessToken, AzureError> {
        let mut cached = self.cached.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(token) = cached.as_ref().filter(|token| !token.needs_refresh()) {
            return Ok(token.clone());
        }
        let token = self.fetch()?;
        *cached = Some(token.clone());
        Ok(token)
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

/// End the tree, then let the reader thread finish: killing every member closes the
/// pipe writers, so the reader returns on its own within the grace period.
fn abandon(child: OwnedChild, receiver: &mpsc::Receiver<std::io::Result<Vec<u8>>>) -> AzureError {
    drop(child);
    let _ = receiver.recv_timeout(READER_GRACE);
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
