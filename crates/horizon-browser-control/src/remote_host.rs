//! The SSH channel of a local client to the browser host of another machine.
//!
//! Two clients reach a profile host: the MCP bridge that serves an agent, and the
//! browser panel of Horizon that shows one page of that host. Both read the
//! record of the running host over the same SSH transport and start the same MCP
//! server on the far side of it, so this module holds that way once: the probe of
//! `profile-host.json`, the command line of the remote server, the bounded calls
//! that the relay makes for its two rewrites, and the channel itself.
//!
//! The calls are one bounded `ssh` process each, with nothing interactive: a
//! target that asks for a password would wait for input that never comes, so a
//! person connects once from a terminal, after which the key answers.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

/// The runtime root of the remote host, as a path on that machine. Unset means
/// the remote default, `$HOME/.horizon`, which is what Horizon leaves it at.
pub const REMOTE_ROOT_ENV: &str = "HORIZON_BROWSER_REMOTE_ROOT";
/// The `horizon-browser` program on the remote machine, when the login shell of
/// the SSH target does not find it on its `PATH`.
pub const REMOTE_EXECUTABLE_ENV: &str = "HORIZON_BROWSER_REMOTE_EXECUTABLE";
/// Longest SSH target that a channel takes. It is one argument of `ssh`, so a
/// short alias is enough for an SSH config name, a `user@host` and an address.
pub const MAX_TARGET_LEN: usize = 255;
/// Longest actor that a channel takes, as a browser page identity allows.
pub const MAX_ACTOR_LEN: usize = 128;
/// Bound of the record that the probe reads.
pub const MAX_PROBE_BYTES: u64 = 64 * 1024;
/// The exit code of the probe when the remote runtime root holds no record.
const PROBE_NO_RECORD: i32 = 3;
/// The remote default runtime root, as the remote shell reads it.
const DEFAULT_ROOT: &str = "\"$HOME/.horizon\"";
/// The program that serves the MCP contract on the remote machine.
const REMOTE_EXECUTABLE: &str = "horizon-browser";
/// The record that a running profile host writes under its runtime root.
const RECORD: &str = "profile-host.json";
const SSH: &str = "ssh";
const READ_BUFFER: usize = 8 * 1024;

/// Why a channel to the browser host of another machine cannot serve its client.
#[derive(Debug, thiserror::Error)]
pub enum RemoteChannelError {
    #[error("remote browser host unavailable: no actor was given, so its pages would have no identity")]
    ActorMissing,
    #[error("`{target}` is not one SSH name that `ssh` can take as one argument")]
    InvalidTarget { target: String },
    #[error("remote browser host {target} is not running: {reason}")]
    HostUnavailable { target: String, reason: String },
    #[error("could not use the SSH target {target}: {reason}")]
    Transport { target: String, reason: String },
}

/// Whether `target` is one name that `ssh` takes as one argument and not as an
/// option of its own.
#[must_use]
pub fn valid_target(target: &str) -> bool {
    let target = target.trim();
    !target.is_empty()
        && target.len() <= MAX_TARGET_LEN
        && !target.starts_with('-')
        && !target.chars().any(char::is_whitespace)
        && !target.chars().any(char::is_control)
}

/// The runtime root of the remote host, when this computer names one.
#[must_use]
pub fn remote_root() -> Option<PathBuf> {
    std::env::var(REMOTE_ROOT_ENV)
        .ok()
        .map(|root| root.trim().to_string())
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
}

/// The program of the remote server, from the environment when the machine keeps
/// it outside the `PATH` of the login shell of the SSH target.
#[must_use]
pub fn remote_executable() -> String {
    std::env::var(REMOTE_EXECUTABLE_ENV)
        .ok()
        .map(|program| program.trim().to_string())
        .filter(|program| !program.is_empty())
        .unwrap_or_else(|| REMOTE_EXECUTABLE.to_string())
}

/// What the record of a running remote host gives a client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteHostRecord {
    /// The runtime root of the remote host, as a path on that machine.
    pub root: PathBuf,
    /// The instance that the remote host wrote in `profile-host.json`.
    pub host_instance: String,
}

/// Read the record of the host that runs at `target`, or why there is none.
///
/// # Errors
/// See [`RemoteChannelError`]: no host runs on that runtime root, the transport
/// failed, or the record is not a record of a running host.
pub fn probe(target: &str, root: Option<&Path>) -> Result<RemoteHostRecord, RemoteChannelError> {
    let output = run(target, &probe_command(root), MAX_PROBE_BYTES).map_err(|error| RemoteChannelError::Transport {
        target: target.to_string(),
        reason: error.to_string(),
    })?;
    if output.code == Some(PROBE_NO_RECORD) && !output.too_large {
        return Err(RemoteChannelError::HostUnavailable {
            target: target.to_string(),
            reason: format!("no {RECORD} under its runtime root"),
        });
    }
    if !output.succeeded() {
        return Err(RemoteChannelError::Transport {
            target: target.to_string(),
            reason: output.stderr,
        });
    }
    parse_probe(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| RemoteChannelError::Transport {
        target: target.to_string(),
        reason: format!("its {RECORD} is not a record of a running host"),
    })
}

/// Read the answer of the probe: the runtime root and the record of the host
/// that runs there.
fn parse_probe(text: &str) -> Option<RemoteHostRecord> {
    let mut lines = text.lines();
    let root = lines.next()?.trim();
    let record = lines.next()?.trim();
    let record: serde_json::Value = serde_json::from_str(record).ok()?;
    let host_instance = record.get("host_instance")?.as_str()?;
    if root.is_empty() || !super::manifest::valid_host_instance(host_instance) {
        return None;
    }
    Some(RemoteHostRecord {
        root: PathBuf::from(root),
        host_instance: host_instance.to_string(),
    })
}

/// The MCP server of the browser host at one SSH target, with the standard
/// streams of its channel. The server starts before this value exists, and its
/// process ends when the channel drops.
pub struct RemoteChannel {
    child: Child,
    target: String,
    root: PathBuf,
}

impl RemoteChannel {
    /// Read the record of the host at `target`, then start the MCP server of
    /// that host with `actor` as the identity of the pages of this client.
    ///
    /// # Errors
    /// `target` is not one SSH name, `actor` is empty or holds a control
    /// character, no host runs at the target, or `ssh` cannot start.
    pub fn open(target: &str, actor: &str) -> Result<Self, RemoteChannelError> {
        let target = target.trim();
        if !valid_target(target) {
            return Err(RemoteChannelError::InvalidTarget {
                target: target.to_string(),
            });
        }
        let actor = actor.trim();
        if actor.is_empty() || actor.len() > MAX_ACTOR_LEN || actor.chars().any(char::is_control) {
            return Err(RemoteChannelError::ActorMissing);
        }
        let root = remote_root();
        let host = probe(target, root.as_deref())?;
        let command = server_command(&remote_executable(), actor, &host.host_instance, root.as_deref());
        let child = Command::new(SSH)
            .args(["-Tq", target, &command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| RemoteChannelError::Transport {
                target: target.to_string(),
                reason: error.to_string(),
            })?;
        Ok(Self {
            child,
            target: target.to_string(),
            root: host.root,
        })
    }

    /// The SSH target of the server, as `ssh` resolves it.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The runtime root of the remote host, as a path on that machine.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Take the standard input of the MCP stream. It is `None` after the first
    /// call, and when the process has none.
    pub fn take_input(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    /// Take the standard output of the MCP stream, as the first call takes the
    /// input.
    pub fn take_output(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// Stop the server of the host and its `ssh` process.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Wait for the process of the channel and give its exit status.
    ///
    /// # Errors
    /// Returns when the process cannot be waited on.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    /// Whether the process of the channel has ended, without waiting for it.
    ///
    /// # Errors
    /// Returns when the operating system cannot report the state of the process.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
}

impl Drop for RemoteChannel {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What one finished `ssh` call gives back. `too_large` says that the call wrote
/// more than the caller asked to read, so its output was stopped early.
pub struct Output {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub too_large: bool,
}

impl Output {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.code == Some(0) && !self.too_large
    }
}

/// Run one command string on the remote machine and read at most `limit` bytes
/// of its standard output. A call that writes more is stopped, so a huge file
/// never fills the memory of this computer.
///
/// # Errors
/// Returns when `ssh` cannot start or its output cannot be read.
pub fn run(target: &str, remote_command: &str, limit: u64) -> io::Result<Output> {
    let mut child = Command::new(SSH)
        .args(["-Tq", target, remote_command])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err(io::Error::other("ssh has no standard output"));
    };
    let mut bytes = Vec::new();
    let mut buffer = [0u8; READ_BUFFER];
    let mut too_large = false;
    loop {
        let read = stdout.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if u64::try_from(bytes.len() + read).unwrap_or(u64::MAX) > limit {
            too_large = true;
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    drop(stdout);
    if too_large {
        let _ = child.kill();
    }
    let finished = child.wait_with_output()?;
    Ok(Output {
        code: finished.status.code(),
        stdout: bytes,
        stderr: String::from_utf8_lossy(&finished.stderr).trim().to_string(),
        too_large,
    })
}

/// Copy one local file to `directory/subdirectory/name` on the remote machine.
/// The local file is the standard input of `ssh`, so this computer never holds
/// its bytes.
///
/// # Errors
/// Returns when the local file or `ssh` cannot be opened.
pub fn stage(target: &str, file: &Path, directory: &str, subdirectory: usize, name: &str) -> io::Result<Output> {
    let input = File::open(file)?;
    let child = Command::new(SSH)
        .args(["-Tq", target, &stage_file_command(directory, subdirectory, name)])
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let output = child.wait_with_output()?;
    Ok(Output {
        code: output.status.code(),
        stdout: Vec::new(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        too_large: false,
    })
}

/// Read at most `limit` bytes of one remote file. More bytes than `limit` come
/// back as `None`, because the caller must not inline a capture of any size.
///
/// # Errors
/// Returns when `ssh` fails, or when the remote host could not read the file.
pub fn read_file(target: &str, path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let output = run(target, &read_file_command(path), limit)?;
    if output.too_large {
        return Ok(None);
    }
    if output.code != Some(0) {
        return Err(io::Error::other(format!(
            "the remote host could not read {} ({})",
            path.display(),
            output.stderr
        )));
    }
    Ok(Some(output.stdout))
}

/// Make the private staging directory of one call on the remote machine, under
/// the staging folder of the runtime root. The call folder must not exist, so
/// two calls can never share a copy.
///
/// # Errors
/// Returns when `ssh` fails, or when the remote host could not make the folder.
pub fn create_directory(target: &str, parent: &str, directory: &str) -> io::Result<()> {
    let output = run(target, &create_directory_command(parent, directory), 0)?;
    if output.succeeded() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the remote host could not make {directory} ({})",
            output.stderr
        )))
    }
}

/// Remove the staging directory of one call and every copy in it.
pub fn remove_directory(target: &str, directory: &str) {
    match run(target, &remove_directory_command(directory), 0) {
        Ok(output) if output.succeeded() => tracing::debug!(directory, "removed the remote staging directory"),
        Ok(output) => tracing::warn!(
            directory,
            reason = %output.stderr,
            "the remote staging directory stays until the next call"
        ),
        Err(error) => tracing::warn!(%error, directory, "the remote staging directory stays until the next call"),
    }
}

/// The remote server of a profile host, with the identity that the caller
/// forwards.
///
/// The actor is the local identity of the client, which the host stamps its
/// pages with. The host instance is the one the remote host wrote in its record,
/// never the instance of this computer.
#[must_use]
pub fn server_command(program: &str, actor: &str, host_instance: &str, root: Option<&Path>) -> String {
    use std::fmt::Write as _;

    let mut command = format!(
        "env HORIZON_BROWSER_ACTOR={} HORIZON_BROWSER_HOST_INSTANCE={}",
        quote(actor),
        quote(host_instance)
    );
    if let Some(root) = root {
        let _ = write!(command, " HORIZON_BROWSER_ROOT={}", quote(&path_text(root)));
    }
    let _ = write!(command, " {} mcp --connect --inline-screenshots", quote(program));
    command
}

/// Prints the runtime root of the remote host, then its record on one line.
///
/// A missing record ends the probe with exit code 3, which says that no host
/// runs on that runtime root. The probe reads no profile, because the host
/// decides which profiles it serves.
#[must_use]
pub fn probe_command(root: Option<&Path>) -> String {
    format!(
        "root={root}; printf '%s\\n' \"$root\"; tr -d '\\n' < \"$root/profile-host.json\" || exit 3; \
         printf '\\n'",
        root = root_expression(root)
    )
}

/// One shell word for the runtime root of the remote host.
#[must_use]
pub fn root_expression(root: Option<&Path>) -> String {
    root.map_or_else(|| DEFAULT_ROOT.to_string(), |root| quote(&path_text(root)))
}

/// Read one file of the remote host. `cat` streams the bytes, so a large file is
/// never held twice.
#[must_use]
pub fn read_file_command(path: &Path) -> String {
    format!("cat {}", quote(&path_text(path)))
}

/// Stage one file into `directory`, from the standard input of `ssh`. The copy
/// takes its final name only after every byte arrived, so an interrupted
/// transfer leaves a `.part` name that the cleanup of the call removes.
#[must_use]
pub fn stage_file_command(directory: &str, subdirectory: usize, name: &str) -> String {
    let folder = quote(&format!("{directory}/{subdirectory}"));
    let staged = quote(&format!("{directory}/{subdirectory}/{name}"));
    format!("set -eu; mkdir -p -- {folder} && cat > {staged}.part && mv -- {staged}.part {staged}")
}

/// Make the staging folder of the runtime root, then the private folder of one
/// call. The call folder must not exist, so an interrupted call can not hand its
/// copies to the next one.
#[must_use]
pub fn create_directory_command(parent: &str, directory: &str) -> String {
    format!(
        "set -eu; umask 077; mkdir -p -- {parent} && mkdir -- {directory}",
        parent = quote(parent),
        directory = quote(directory)
    )
}

/// Remove the staging directory of one call and every staged copy in it. `--`
/// keeps a folder name that begins with a dash a name and not an option.
#[must_use]
pub fn remove_directory_command(directory: &str) -> String {
    format!("rm -rf -- {}", quote(directory))
}

/// A value as the remote shell must read it: one word, with no expansion.
#[must_use]
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
