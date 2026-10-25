//! The `ssh` side of the bridge: one bounded call at a time to the remote
//! machine, and nothing else. `ssh` carries the MCP stream of the server and the
//! bytes of the files that cross the boundary; the bridge adds no listener and
//! holds no credential.
//!
//! The bridge runs inside an agent turn, so every call is non-interactive: a
//! target that asks for a password would wait for input that never comes. The
//! first connection of a person therefore runs once from a terminal, after which
//! the key answers the bridge.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;
use std::process::{Command, Stdio};

use super::command;

const SSH: &str = "ssh";
const READ_BUFFER: usize = 8 * 1024;

/// What one finished `ssh` call gives back. `too_large` says that the call wrote
/// more than the caller asked to read, so its output was stopped early.
pub(super) struct Output {
    pub(super) code: Option<i32>,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: String,
    pub(super) too_large: bool,
}

impl Output {
    pub(super) fn succeeded(&self) -> bool {
        self.code == Some(0) && !self.too_large
    }
}

/// Start the remote MCP server of the bridge. Its standard streams carry the
/// MCP stream, and its diagnostics join this process's standard error.
pub(super) fn server(target: &str, remote_command: &str) -> io::Result<std::process::Child> {
    Command::new(SSH)
        .args(["-Tq", target, remote_command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
}

/// Run one command string on the remote machine and read at most `limit` bytes
/// of its standard output. A call that writes more is stopped, so a huge file
/// never fills the memory of the bridge.
pub(super) fn run(target: &str, remote_command: &str, limit: u64) -> io::Result<Output> {
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
/// The local file is the standard input of `ssh`, so the bridge never holds its
/// bytes.
pub(super) fn stage(target: &str, file: &Path, directory: &str, subdirectory: usize, name: &str) -> io::Result<Output> {
    let input = File::open(file)?;
    let child = Command::new(SSH)
        .args([
            "-Tq",
            target,
            &command::stage_file_command(directory, subdirectory, name),
        ])
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
pub(super) fn read_file(target: &str, path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let output = run(target, &command::read_file_command(path), limit)?;
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
pub(super) fn create_directory(target: &str, parent: &str, directory: &str) -> io::Result<()> {
    let output = run(target, &command::create_directory_command(parent, directory), 0)?;
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
pub(super) fn remove_directory(target: &str, directory: &str) {
    match run(target, &command::remove_directory_command(directory), 0) {
        Ok(output) if output.succeeded() => tracing::debug!(directory, "removed the remote staging directory"),
        Ok(output) => tracing::warn!(
            directory,
            reason = %output.stderr,
            "the remote staging directory stays until the next call"
        ),
        Err(error) => tracing::warn!(%error, directory, "the remote staging directory stays until the next call"),
    }
}
