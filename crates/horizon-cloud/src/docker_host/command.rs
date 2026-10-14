//! Runs docker on a host, each call bounded in time and cancellable.
use super::{Access, Host, PRIVATE_CONFIG};
use crate::{Cancellation, CloudError, Reason};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
const ERROR_LIMIT: u64 = 64 * 1024;
/// How long the output of a finished command may take to arrive, for a command that
/// left a descendant holding its pipes open.
const DRAIN: Duration = Duration::from_secs(2);
const TIMED_OUT: &str = "The Docker host did not answer in time; check the host and try again";

/// Why a docker call failed.
#[derive(Debug)]
pub enum Failure {
    /// Docker ran and refused, with what it wrote to standard error.
    Refused(String),
    /// The call could not run, ran out of time or was cancelled.
    Error(CloudError),
}

impl From<CloudError> for Failure {
    fn from(error: CloudError) -> Self {
        Self::Error(error)
    }
}

impl From<Failure> for CloudError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Refused(text) => Self::Rejected(Reason::from_text(&text, "")),
            Failure::Error(error) => error,
        }
    }
}

/// Runs docker on one host.
pub trait Docker {
    /// Runs `docker <args>` within `timeout` and returns its standard output.
    /// # Errors
    /// [`Failure::Refused`] when docker ran and failed, else [`Failure::Error`].
    fn docker(&self, args: &[String], timeout: Duration) -> Result<String, Failure>;
}

/// A saved host's engine, reached as the host's access says.
pub struct Engine<'a> {
    pub host: &'a Host,
    /// The private Docker configuration directory for this computer's engine.
    pub local_config: PathBuf,
    pub cancel: &'a Cancellation,
}

impl Docker for Engine<'_> {
    fn docker(&self, args: &[String], timeout: Duration) -> Result<String, Failure> {
        // A cancelled operation sends nothing more, also between its calls.
        self.cancel.check()?;
        if self.host.access == Access::ThisPc {
            // The worker's loopback must be this computer's, as its record says.
            if std::env::var("DOCKER_HOST").is_ok_and(|engine| !engine.is_empty() && !engine.starts_with("unix://")) {
                return Err(
                    CloudError::Invalid("DOCKER_HOST names another computer's engine; save that host instead").into(),
                );
            }
            private_config(&self.local_config)
                .map_err(|_| CloudError::Invalid("The private Docker configuration could not be written"))?;
        }
        let mut command = self.host.command(args, &self.local_config)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|_| CloudError::Invalid("docker or ssh could not be started on this computer"))?;
        let stdout = drain(child.stdout.take(), OUTPUT_LIMIT);
        let stderr = drain(child.stderr.take(), ERROR_LIMIT);
        let deadline = Instant::now() + timeout;
        let status: ExitStatus = loop {
            if let Some(status) = child.try_wait().map_err(|_| CloudError::Invalid(TIMED_OUT))? {
                break status;
            }
            if self.cancel.is_cancelled() || Instant::now() >= deadline {
                // Killing the local command ends an SSH session too; Docker may still
                // finish a request it already received, so callers check again.
                let _ = child.kill();
                let _ = child.wait();
                self.cancel.check()?;
                return Err(CloudError::Invalid(TIMED_OUT).into());
            }
            thread::sleep(Duration::from_millis(20));
        };
        let output = stdout.recv_timeout(DRAIN).ok().flatten();
        let errors = stderr.recv_timeout(DRAIN).ok().flatten().unwrap_or_default();
        if !status.success() {
            // OpenSSH exits with 255 when it cannot reach or log in to the host.
            if self.host.access != Access::ThisPc && status.code() == Some(255) {
                return Err(Failure::Error(CloudError::Transport));
            }
            return Err(Failure::Refused(errors));
        }
        output.ok_or(Failure::Error(CloudError::InvalidResponse))
    }
}

fn private_config(directory: &Path) -> std::io::Result<()> {
    let file = directory.join("config.json");
    if !file.try_exists()? {
        // Written whole before it appears, as another call may read it at once.
        std::fs::create_dir_all(directory)?;
        let mut pending = tempfile::NamedTempFile::new_in(directory)?;
        std::io::Write::write_all(&mut pending, PRIVATE_CONFIG.as_bytes())?;
        pending.persist(file).map_err(|error| error.error)?;
    }
    Ok(())
}

/// Reads `pipe` to its end on its own thread; None when it holds more than `limit` bytes.
fn drain(pipe: Option<impl Read + Send + 'static>, limit: u64) -> mpsc::Receiver<Option<String>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = pipe.map(|mut pipe| {
            let read = (&mut pipe).take(limit + 1).read_to_end(&mut bytes);
            // The rest is discarded, so a command with too much output still ends.
            std::io::copy(&mut pipe, &mut std::io::sink()).and(read)
        });
        let complete = read.is_none_or(|read| read.is_ok()) && bytes.len() as u64 <= limit;
        let _ = sender.send(complete.then(|| String::from_utf8_lossy(&bytes).into_owned()));
    });
    receiver
}
