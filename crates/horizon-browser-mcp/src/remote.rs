//! The MCP stream of a profile host that runs on another computer, over SSH.
//!
//! `horizon-browser mcp --connect --inline-screenshots --remote <target>` and
//! `horizon --browser-mcp --remote <target>` run on the computer of the person.
//! They start `ssh -Tq <target> horizon-browser mcp --connect
//! --inline-screenshots` on the machine of the profile host and forward the
//! JSON-RPC lines of both directions. The pages, the profiles and the cookies
//! stay on that machine, and the person signs in there one time.
//!
//! The bridge reads the record of the remote host before it forwards a line, so
//! it forwards the host instance that the remote host wrote, never the instance
//! of this computer. The actor of the agent stays the local identity, because a
//! page is stamped for the actor that opened it.
//!
//! Two results cannot cross an SSH stream as a path. The bridge therefore stages
//! the files of a `set_files` action on the remote machine before the action, and
//! adds the bounded bytes of a finished `browser_video` or `browser_network`
//! capture to its result. `mod relay` holds both rewrites, and `mod ssh` holds
//! every call that the bridge makes to the remote machine.

mod command;
mod relay;
mod ssh;

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use horizon_browser_control::manifest;

use relay::Relay;

/// The identity of the calling agent panel. The bridge forwards it, because the
/// remote host stamps each page for the actor that opened it.
pub const ACTOR_ENV: &str = "HORIZON_BROWSER_ACTOR";
/// The runtime root of the remote host, as a path on that machine. Unset means
/// the remote default, `$HOME/.horizon`, which is what Horizon leaves it at.
pub const REMOTE_ROOT_ENV: &str = "HORIZON_BROWSER_REMOTE_ROOT";
/// The `horizon-browser` program on the remote machine, when the login shell of
/// the SSH target does not find it on its `PATH`.
pub const REMOTE_EXECUTABLE_ENV: &str = "HORIZON_BROWSER_REMOTE_EXECUTABLE";
/// The folder under the remote runtime root that holds the staged copies of one
/// call. The bridge removes a call's folder when its answer arrives.
pub const STAGING_DIR: &str = "browser-staging";
/// Largest capture that the bridge adds inline, the same bound as an inline
/// screenshot. A larger capture stays a path on the remote machine.
pub const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;
/// Largest total of the files that one attachment call stages on the remote
/// machine. Each single file keeps the local attachment limit.
pub const MAX_STAGED_BYTES: u64 = 512 * 1024 * 1024;

/// The program that serves the MCP contract on the remote machine.
const REMOTE_EXECUTABLE: &str = "horizon-browser";
/// The record that a running profile host writes under its runtime root.
const RECORD: &str = "profile-host.json";
/// The folder-name prefix of each browser profile of a profile host.
const PROFILE_PREFIX: &str = "profile-";
/// Bound of the record and the profile list that the bridge reads first.
const MAX_PROBE_BYTES: u64 = 64 * 1024;
/// The exit code of the probe when the remote runtime root holds no record.
const PROBE_NO_RECORD: i32 = 3;

/// Why the bridge cannot serve the MCP stream of a remote browser host.
#[derive(Debug, thiserror::Error)]
pub enum RemoteBridgeError {
    #[error("remote browser host unavailable: {ACTOR_ENV} is not set, so the bridge has no actor to forward")]
    ActorUnavailable,
    #[error("remote browser host {target} is not running: {reason}")]
    HostUnavailable { target: String, reason: String },
    #[error("remote browser host {target} does not serve the profile {profile}: {reason}")]
    ProfileUnavailable {
        target: String,
        profile: String,
        reason: String,
    },
    #[error("could not use the SSH target {target}: {reason}")]
    Transport { target: String, reason: String },
}

/// Serve the MCP contract of the profile host at the SSH target `target` until
/// the client closes the stream or the remote server stops.
///
/// `profile` names the browser profile whose pages the callers use. The bridge
/// refuses a profile that the remote host does not serve, so a person does not
/// get an empty page list with no reason.
///
/// # Errors
/// Returns when this computer has no agent identity, when no host runs at
/// `target`, when `profile` is not served, or when the stream fails.
pub fn serve(target: &str, profile: Option<&str>) -> Result<(), RemoteBridgeError> {
    let actor = std::env::var(ACTOR_ENV)
        .ok()
        .map(|actor| actor.trim().to_string())
        .filter(|actor| !actor.is_empty())
        .ok_or(RemoteBridgeError::ActorUnavailable)?;
    let root = std::env::var(REMOTE_ROOT_ENV)
        .ok()
        .map(|root| root.trim().to_string())
        .filter(|root| !root.is_empty())
        .map(PathBuf::from);
    let host = probe(target, root.as_deref())?;
    if let Some(profile) = profile.map(str::trim).filter(|profile| !profile.is_empty())
        && !host.profiles.iter().any(|served| served == profile)
    {
        return Err(RemoteBridgeError::ProfileUnavailable {
            target: target.to_string(),
            profile: profile.to_string(),
            reason: format!(
                "its host file names [{}]",
                if host.profiles.is_empty() {
                    "no profile".to_string()
                } else {
                    host.profiles.join(", ")
                }
            ),
        });
    }
    let command = command::server_command(&remote_executable(), &actor, &host.host_instance, root.as_deref());
    let mut child = ssh::server(target, &command).map_err(|error| RemoteBridgeError::Transport {
        target: target.to_string(),
        reason: error.to_string(),
    })?;
    let relay = Arc::new(Relay::new(target.to_string(), host.root));
    let output = Arc::new(Mutex::new(io::stdout()));
    let responses = {
        let relay = Arc::clone(&relay);
        let output = Arc::clone(&output);
        let stdout = child.stdout.take();
        std::thread::spawn(move || match stdout {
            Some(stdout) => relay::forward_responses(io::BufReader::new(stdout), &mut SharedOut::new(&output), &relay),
            None => Ok(()),
        })
    };
    let stdin = io::stdin();
    let forwarded = child.stdin.take().map_or(Ok(()), |server| {
        relay::forward_requests(
            io::BufReader::new(stdin.lock()),
            server,
            &mut SharedOut::new(&output),
            &relay,
        )
    });
    let status = child.wait();
    let answered = responses.join().unwrap_or(Ok(()));
    relay.remove_leftover_staging();
    forwarded.and(answered).map_err(|error| RemoteBridgeError::Transport {
        target: target.to_string(),
        reason: error.to_string(),
    })?;
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(RemoteBridgeError::Transport {
            target: target.to_string(),
            reason: format!("the remote MCP server ended with {status}"),
        }),
        Err(error) => Err(RemoteBridgeError::Transport {
            target: target.to_string(),
            reason: error.to_string(),
        }),
    }
}

/// The program of the remote server, from the environment when the machine keeps
/// it outside the `PATH` of the login shell of the SSH target.
fn remote_executable() -> String {
    std::env::var(REMOTE_EXECUTABLE_ENV)
        .ok()
        .map(|program| program.trim().to_string())
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| REMOTE_EXECUTABLE.to_string())
}

/// What the record of a running remote host gives the bridge.
struct RemoteHost {
    /// The runtime root of the remote host, as a path on that machine.
    root: PathBuf,
    /// The instance that the remote host wrote in `profile-host.json`.
    host_instance: String,
    /// The browser profiles that the host serves.
    profiles: Vec<String>,
}

fn probe(target: &str, root: Option<&std::path::Path>) -> Result<RemoteHost, RemoteBridgeError> {
    let output = ssh::run(target, &command::probe_command(root), MAX_PROBE_BYTES).map_err(|error| {
        RemoteBridgeError::Transport {
            target: target.to_string(),
            reason: error.to_string(),
        }
    })?;
    if output.code == Some(PROBE_NO_RECORD) && !output.too_large {
        return Err(RemoteBridgeError::HostUnavailable {
            target: target.to_string(),
            reason: format!("no {RECORD} under its runtime root"),
        });
    }
    if !output.succeeded() {
        return Err(RemoteBridgeError::Transport {
            target: target.to_string(),
            reason: output.stderr,
        });
    }
    parse_probe(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| RemoteBridgeError::Transport {
        target: target.to_string(),
        reason: format!("its {RECORD} is not a record of a running host"),
    })
}

/// Read the answer of the probe: the runtime root, the record, then one folder
/// name for each browser profile.
fn parse_probe(text: &str) -> Option<RemoteHost> {
    let mut lines = text.lines();
    let root = lines.next()?.trim();
    let record = lines.next()?.trim();
    let record: serde_json::Value = serde_json::from_str(record).ok()?;
    let host_instance = record.get("host_instance")?.as_str()?;
    if root.is_empty() || !manifest::valid_host_instance(host_instance) {
        return None;
    }
    let profiles = lines
        .filter_map(|line| line.trim().strip_prefix(PROFILE_PREFIX))
        .filter(|profile| !profile.is_empty())
        .map(str::to_string)
        .collect();
    Some(RemoteHost {
        root: PathBuf::from(root),
        host_instance: host_instance.to_string(),
        profiles,
    })
}

/// The one writer of the MCP stream to the client. The two forwarding threads
/// share it, so an answer and a refusal can never mix their bytes.
struct SharedOut(Arc<Mutex<io::Stdout>>);

impl SharedOut {
    fn new(stdout: &Arc<Mutex<io::Stdout>>) -> Self {
        Self(Arc::clone(stdout))
    }
}

impl io::Write for SharedOut {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut stdout = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        stdout.write_all(buffer)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).flush()
    }
}

#[cfg(test)]
mod tests;
