//! Capability selection travels in the environment so legacy checker argv stays valid.
use super::{Error, Result};
use horizon_cloud::Capabilities;

pub(super) const CAPABILITIES_ENV: &str = "HORIZON_WORKER_CAPABILITIES";
const CAPABILITIES_MARKER: &str = "horizon-capabilities-contract=1";
const SESSION_RESTART_MARKER: &str = "horizon-session-restart-contract=1";
const CONTAINER_STARTED_MARKER: &str = "horizon-container-started=";
const LAST_SELF_STOP_MARKER: &str = "horizon-last-self-stop=";
const SELF_STOP_MARKER: &str = "horizon-self-stop-contract=1";
/// A reason longer than this was not written by `horizon-worker-stop`.
const SELF_STOP_REASON_LIMIT: usize = 200;

/// A stop an agent asked for on its worker, with the reason it gave and who asked.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct SelfStop {
    /// Milliseconds since the epoch by the worker's clock.
    pub at: u64,
    pub reason: String,
    /// The requesting session's agent, such as `claude`, when the worker knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The requesting Horizon session, as the watcher identified it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl SelfStop {
    fn well_formed(mut self) -> Option<Self> {
        let reason_ok = !self.reason.trim().is_empty()
            && self.reason.chars().count() <= SELF_STOP_REASON_LIMIT
            && !self.reason.chars().any(char::is_control);
        // Older records and unknown requesters carry empty names; keep only plain ones.
        self.agent = self
            .agent
            .filter(|agent| !agent.is_empty() && agent.len() <= 20 && agent.chars().all(|c| c.is_ascii_alphanumeric()));
        self.session = self.session.filter(|session| {
            !session.is_empty()
                && session.len() <= 100
                && session
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        });
        reason_ok.then_some(self)
    }
}

/// Optional worker features the checker reports beside the required markers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkerContract {
    /// `horizon-worker-session --relaunch` can replace a session process lost in a
    /// container reset, in its existing worktree. Older images report such sessions lost.
    pub session_restart: bool,
    /// When the worker's container started, by the worker's clock. Older images do not report it.
    pub container_started: Option<std::time::SystemTime>,
    /// The newest stop an agent asked for on this worker. Older images do not report it.
    pub last_self_stop: Option<SelfStop>,
    /// The image supports agent stops, so a missing `last_self_stop` means none was recorded.
    pub self_stop_reported: bool,
}

impl WorkerContract {
    pub(super) fn reported(output: &str) -> Self {
        Self {
            session_restart: reports(output, SESSION_RESTART_MARKER),
            container_started: output
                .lines()
                .find_map(|line| line.strip_prefix(CONTAINER_STARTED_MARKER))
                .filter(|millis| !millis.is_empty() && millis.bytes().all(|b| b.is_ascii_digit()))
                .and_then(|millis| millis.parse().ok())
                .and_then(|millis| std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_millis(millis))),
            last_self_stop: output
                .lines()
                .find_map(|line| line.strip_prefix(LAST_SELF_STOP_MARKER))
                .and_then(|json| serde_json::from_str::<SelfStop>(json).ok())
                .and_then(SelfStop::well_formed),
            self_stop_reported: reports(output, SELF_STOP_MARKER),
        }
    }
}

fn reports(output: &str, marker: &str) -> bool {
    output.lines().any(|line| line == marker)
}

pub(super) fn environment(capabilities: &Capabilities) -> Result<String> {
    serde_json::to_string(capabilities)
        .map(|json| format!("{CAPABILITIES_ENV}={json}"))
        .map_err(|_| Error::Json)
}

pub(super) fn validate(output: &str, capabilities: &Capabilities, git_auth: bool, idle_stop: bool) -> Result<()> {
    let contains = |marker| reports(output, marker);
    if !contains("horizon-source-contract=1") {
        return Err(Error::Invalid(
            "Worker image does not support committed source dependencies",
        ));
    }
    if capabilities != &Capabilities::default() && !contains(CAPABILITIES_MARKER) {
        return Err(Error::Invalid(
            "Worker image cannot validate selected capabilities; rebuild with the current worker bootstrap",
        ));
    }
    if capabilities.browserstack.is_some() && !contains("horizon-browserstack-contract=1") {
        return Err(Error::Invalid(
            "Worker image lacks requested remote-browser support; rebuild before deployment",
        ));
    }
    if git_auth && !contains("horizon-git-auth-contract=1") {
        return Err(Error::Invalid("Worker image does not support Git credential transfer"));
    }
    // An older supervisor ignores the idle period, so the worker would never stop.
    if idle_stop && !contains("horizon-idle-stop-contract=1") {
        return Err(Error::Invalid(
            "Worker image does not support idle_stop_minutes; rebuild with the current worker bootstrap",
        ));
    }
    Ok(())
}

pub(super) fn readiness_command(capabilities: &Capabilities) -> Result<String> {
    let environment = environment(capabilities)?.replace('\'', "'\\''");
    // Modern checkers must still verify active services; legacy full checkers have
    // no readiness flag. Exact marker matching avoids accepting incidental output.
    Ok(format!(
        "export '{environment}'; contract=$(horizon-worker-check) || exit $?; \
         newline='\n'; case \"$newline$contract$newline\" in \
         *\"${{newline}}{CAPABILITIES_MARKER}${{newline}}\"*) exec horizon-worker-check --ready ;; \
         *) printf '%s\\n' \"$contract\" ;; esac"
    ))
}

#[cfg(test)]
mod tests;
