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
//! capture to its result. `mod relay` holds both rewrites.
//!
//! The channel itself is not the bridge's own: a browser panel of Horizon reaches
//! a profile host through the same record, the same remote command and the same
//! bounded `ssh` calls, so `horizon_browser_control::remote_host` holds that one
//! implementation and both clients call it.

mod relay;

use std::io;
use std::sync::{Arc, Mutex};

use horizon_browser_control::remote_host::{RemoteChannel, RemoteChannelError};

use relay::Relay;

/// The identity of the calling agent panel. The bridge forwards it, because the
/// remote host stamps each page for the actor that opened it.
pub const ACTOR_ENV: &str = "HORIZON_BROWSER_ACTOR";
pub use horizon_browser_control::remote_host::{REMOTE_EXECUTABLE_ENV, REMOTE_ROOT_ENV};
/// The folder under the remote runtime root that holds the staged copies of one
/// call. The bridge removes a call's folder when its answer arrives.
pub const STAGING_DIR: &str = "browser-staging";
/// Largest capture that the bridge adds inline, the same bound as an inline
/// screenshot. A larger capture stays a path on the remote machine.
pub const MAX_INLINE_BYTES: u64 = 8 * 1024 * 1024;
/// Largest total of the files that one attachment call stages on the remote
/// machine. Each single file keeps the local attachment limit.
pub const MAX_STAGED_BYTES: u64 = 512 * 1024 * 1024;

/// Why the bridge cannot serve the MCP stream of a remote browser host.
#[derive(Debug, thiserror::Error)]
pub enum RemoteBridgeError {
    #[error("remote browser host unavailable: {ACTOR_ENV} is not set, so the bridge has no actor to forward")]
    ActorUnavailable,
    #[error("remote browser host unavailable: no actor was given, so its pages would have no identity")]
    ActorMissing,
    #[error("`{target}` is not one SSH name that `ssh` can take as one argument")]
    InvalidTarget { target: String },
    #[error("remote browser host {target} is not running: {reason}")]
    HostUnavailable { target: String, reason: String },
    #[error("could not use the SSH target {target}: {reason}")]
    Transport { target: String, reason: String },
}

impl From<RemoteChannelError> for RemoteBridgeError {
    fn from(error: RemoteChannelError) -> Self {
        match error {
            RemoteChannelError::ActorMissing => Self::ActorMissing,
            RemoteChannelError::InvalidTarget { target } => Self::InvalidTarget { target },
            RemoteChannelError::HostUnavailable { target, reason } => Self::HostUnavailable { target, reason },
            RemoteChannelError::Transport { target, reason } => Self::Transport { target, reason },
            // The bridge relays one stream, so a host that stops answering ends
            // as a transport failure with the bound in its reason.
            RemoteChannelError::NoAnswer { target, seconds } => Self::Transport {
                target,
                reason: format!("the host gave no answer within {seconds} seconds"),
            },
        }
    }
}

/// Serve the MCP contract of the profile host at the SSH target `target` until
/// the client closes the stream or the remote server stops.
///
/// `profile` is the profile that a `browser_create` opens when the call names no
/// `target`, so every page of an agent lands in the profile that the person
/// chose without the agent knowing its name. A create that names its own target
/// keeps it. The bridge does not check the profile against the host: the host
/// decides which profiles it serves, and it refuses an unknown one with its own
/// `target_unknown`, which crosses unchanged.
///
/// # Errors
/// Returns when this computer has no agent identity, when no host runs at
/// `target`, or when the stream fails.
pub fn serve(target: &str, profile: Option<&str>) -> Result<(), RemoteBridgeError> {
    let actor = std::env::var(ACTOR_ENV)
        .ok()
        .map(|actor| actor.trim().to_string())
        .filter(|actor| !actor.is_empty())
        .ok_or(RemoteBridgeError::ActorUnavailable)?;
    let mut channel = RemoteChannel::open(target, &actor).map_err(RemoteBridgeError::from)?;
    let profile = profile.map(str::trim).filter(|profile| !profile.is_empty());
    let relay = Arc::new(Relay::new(
        target.to_string(),
        channel.root().to_path_buf(),
        profile.map(str::to_string),
    ));
    let output = Arc::new(Mutex::new(io::stdout()));
    let responses = {
        let relay = Arc::clone(&relay);
        let output = Arc::clone(&output);
        let stdout = channel.take_output();
        std::thread::spawn(move || match stdout {
            Some(stdout) => relay::forward_responses(io::BufReader::new(stdout), &mut SharedOut::new(&output), &relay),
            None => Ok(()),
        })
    };
    let stdin = io::stdin();
    let forwarded = channel.take_input().map_or(Ok(()), |server| {
        relay::forward_requests(
            io::BufReader::new(stdin.lock()),
            server,
            &mut SharedOut::new(&output),
            &relay,
        )
    });
    let status = channel.wait();
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
