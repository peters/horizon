//! The ARM run-command channel: submit one of the closed set of commands, follow the
//! `Azure-AsyncOperation` status resource only under the owning subscription, and
//! unwrap the command's stdout.
use super::{AzureArmHttp, AzureError, COMPUTE_API_VERSION, MANAGEMENT_ENDPOINT, decode_json, text};
use std::time::{Duration, Instant};

/// Polling schedule for the run-command async operation once it was accepted; ARM's
/// `Retry-After` guidance lengthens a step, never shortens it.
const RUN_COMMAND_BACKOFF_MS: [u64; 9] = [500, 1_000, 2_000, 4_000, 8_000, 15_000, 30_000, 30_000, 30_000];
/// Absolute bound on that polling, sleeps and requests included.
const RUN_COMMAND_BOUND: Duration = Duration::from_secs(120);
/// Longest single wait a `Retry-After` header may ask for.
const RETRY_AFTER_CAP: Duration = Duration::from_secs(60);
/// Longest operation URL followed. ARM signs its async-operation URLs with a
/// certificate carried in the query string (`c=`), which alone runs to about 3 KB, so
/// the live URL is well over 3 KB; 8 KB leaves room without accepting arbitrary sizes.
const OPERATION_URL_LIMIT: usize = 8_192;

/// The only scripts the adapter ever executes inside a worker; there is no way to pass
/// arbitrary text to the run-command channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AzureRunCommand {
    /// Print the host public key the worker container's SSH server is serving, from
    /// `/etc/ssh` inside the fixed `horizon-worker` container: the retained candidate
    /// on the data disk exists before the server does, so only the materialised key
    /// proves the endpoint is up with this identity.
    HostKey,
}

impl AzureRunCommand {
    #[must_use]
    pub const fn script(self) -> &'static str {
        match self {
            Self::HostKey => "docker exec horizon-worker cat /etc/ssh/ssh_host_ed25519_key.pub",
        }
    }
}

impl AzureArmHttp {
    /// Execute one closed command inside the VM through ARM and return its stdout, or
    /// `None` when the VM does not exist.
    pub(super) fn submit_run_command(
        &self,
        group: &str,
        name: &str,
        command: AzureRunCommand,
    ) -> Result<Option<String>, AzureError> {
        let operation = "virtual machine run command";
        let url = self.resource_url(
            group,
            "Microsoft.Compute/virtualMachines",
            name,
            COMPUTE_API_VERSION,
            "/runCommand",
        )?;
        let authorization = self.authorization()?;
        let body = serde_json::json!({ "commandId": "RunShellScript", "script": [command.script()] });
        let response = self
            .agent
            .post(&url)
            .header("Authorization", &authorization)
            .send_json(&body)
            .map_err(|_| AzureError::RequestFailed { operation })?;
        let status = response.status().as_u16();
        let value = match status {
            404 => return Ok(None),
            200 => decode_json(response, &[200], operation)?,
            // Only the Azure-AsyncOperation status resource is followed: it reports a
            // JSON status with the command output. A Location-only answer would need
            // the final-resource protocol (202 while running), which Compute does not
            // use for run commands, so it is refused rather than half-implemented.
            202 => {
                let poll = response
                    .headers()
                    .get("azure-asyncoperation")
                    .and_then(|header| header.to_str().ok())
                    .filter(|poll| self.owns_operation_url(poll))
                    .ok_or(AzureError::InvalidResponse { operation })?
                    .to_string();
                self.await_operation(&poll, operation, retry_after_header(response.headers()))?
            }
            status => return Err(AzureError::UnexpectedStatus { operation, status }),
        };
        run_command_stdout(&value, operation).map(Some)
    }

    /// Only an operation URL under this subscription on the management endpoint is
    /// polled, and only one whose path cannot be normalised out of it: every segment
    /// after the subscription is a plain token (no dot segments, no percent-encoding, no
    /// backslashes or other separators) and the query carries only the shape ARM uses
    /// (the API version plus its signature parameters, all base64url or plain tokens).
    fn owns_operation_url(&self, url: &str) -> bool {
        let prefix = format!("{MANAGEMENT_ENDPOINT}/subscriptions/{}/", self.subscription_id);
        let Some(rest) = url.strip_prefix(&prefix) else {
            return false;
        };
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let plain_segment = |segment: &str| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        };
        url.len() <= OPERATION_URL_LIMIT
            && path.split('/').all(plain_segment)
            && query
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'=' | b'&'))
    }

    /// Poll an async operation until it is terminal, within the run-command bound:
    /// an absolute deadline covers the sleeps, every request is capped at what is left
    /// of it, and ARM's `Retry-After` (from the acceptance and from every poll) is
    /// honoured, so a throttled poll waits instead of failing the command.
    fn await_operation(
        &self,
        url: &str,
        operation: &'static str,
        mut retry_after: Option<Duration>,
    ) -> Result<serde_json::Value, AzureError> {
        let deadline = Instant::now() + RUN_COMMAND_BOUND;
        for delay_ms in RUN_COMMAND_BACKOFF_MS {
            let Some(left) = super::remaining(deadline) else {
                break;
            };
            let delay = Duration::from_millis(delay_ms).max(retry_after.unwrap_or_default());
            if !cfg!(test) {
                std::thread::sleep(delay.min(left));
            }
            let Some(budget) = super::remaining(deadline) else {
                break;
            };
            let response = self.get_within(url, operation, budget)?;
            retry_after = retry_after_header(response.headers());
            let value = match response.status().as_u16() {
                429 => continue,
                404 => return Err(AzureError::InvalidResponse { operation }),
                _ => decode_json(response, &[200], operation)?,
            };
            match text(&value, "/status").as_deref() {
                Some("Succeeded") => return Ok(value),
                Some("Failed" | "Canceled") => return Err(AzureError::RequestFailed { operation }),
                _ => {}
            }
        }
        Err(AzureError::OperationTimedOut { operation })
    }
}

/// `Retry-After` in whole seconds, capped; the HTTP-date form is not used by ARM's
/// async-operation resources and is ignored.
fn retry_after_header(headers: &ureq::http::HeaderMap) -> Option<Duration> {
    headers
        .get("retry-after")
        .and_then(|header| header.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds).min(RETRY_AFTER_CAP))
}

/// Standard output of a finished run command, taken from every status record it
/// returned. Fails closed: a record that is not a success, an unknown record kind, any
/// standard error, and a missing or ambiguous standard output are all errors, so a
/// half-failed command can never be promoted to a trusted result.
fn run_command_stdout(value: &serde_json::Value, operation: &'static str) -> Result<String, AzureError> {
    let invalid = AzureError::InvalidResponse { operation };
    let records = value
        .pointer("/properties/output/value")
        .or_else(|| value.pointer("/value"))
        .and_then(serde_json::Value::as_array)
        .filter(|records| !records.is_empty())
        .ok_or(invalid.clone())?;
    let (mut stdout, mut stderr) = (None, String::new());
    for record in records {
        let code = text(record, "/code").ok_or(invalid.clone())?;
        let (kind, state) = code.rsplit_once('/').ok_or(invalid.clone())?;
        if !state.eq_ignore_ascii_case("succeeded") {
            return Err(AzureError::RequestFailed { operation });
        }
        let message = text(record, "/message").unwrap_or_default();
        let output = match kind {
            // The classic action wraps both streams in one message; both delimiters
            // must be present, or nothing proves standard error was empty.
            "ProvisioningState" => {
                let (_, streams) = message.split_once("[stdout]").ok_or(invalid.clone())?;
                let (out, err) = streams.split_once("[stderr]").ok_or(invalid.clone())?;
                stderr.push_str(err);
                out.to_string()
            }
            "ComponentStatus/StdOut" => message,
            "ComponentStatus/StdErr" => {
                stderr.push_str(&message);
                continue;
            }
            _ => return Err(invalid),
        };
        if stdout.replace(output).is_some() {
            return Err(invalid);
        }
    }
    if !stderr.trim().is_empty() {
        return Err(AzureError::RequestFailed { operation });
    }
    stdout.map(|out| out.trim().to_string()).ok_or(invalid)
}
