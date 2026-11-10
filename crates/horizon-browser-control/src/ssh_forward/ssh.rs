//! One `ssh` program with the bounds of a connect.
//!
//! This computer reads the endpoint of the browser machine and starts the
//! forward of that machine with the same `ssh`. The value below carries that
//! program and the bounds of one call, so a caller hands one value down and a
//! test replaces the program, the bound and the readiness probe at once,
//! instead of opening a connection.

use std::sync::Arc;
use std::time::Duration;

use super::transport::ssh_program;
use super::{READY_DEADLINE, REMOTE_OUTPUT_LIMIT, SshForward, SshForwardError, accepts_connection, run_remote_with};

/// The `ssh` of this computer, or the stand-in program of a test.
#[derive(Clone)]
pub struct Ssh {
    program: String,
    forward_deadline: Duration,
    probe: Arc<dyn Fn(u16) -> bool + Send + Sync>,
}

impl Default for Ssh {
    fn default() -> Self {
        Self {
            program: ssh_program(),
            forward_deadline: READY_DEADLINE,
            probe: Arc::new(accepts_connection),
        }
    }
}

impl Ssh {
    /// Forward one free loopback port of this computer to the `DevTools` port
    /// `remote_port` of `target`, and wait until the forward takes a connection.
    ///
    /// # Errors
    /// See [`SshForwardError`]: the target is not one SSH name, `ssh` cannot
    /// start, it ends before the forward listens (a refused host key names
    /// itself in the reason), or no connection arrives in time.
    pub fn start_forward(&self, target: &str, remote_port: u16) -> Result<SshForward, SshForwardError> {
        SshForward::start_with(target, remote_port, &self.program, self.forward_deadline, |port| {
            (self.probe)(port)
        })
    }

    /// Run one command on `target` and give its standard output.
    ///
    /// # Errors
    /// See [`SshForwardError`]: the target is not one SSH name, `ssh` cannot
    /// start or fails (its own words are the reason), or the command writes
    /// nothing before `deadline`.
    pub fn run_remote(&self, target: &str, command: &str, deadline: Duration) -> Result<String, SshForwardError> {
        run_remote_with(target, command, &self.program, REMOTE_OUTPUT_LIMIT, deadline)
    }

    /// The settings of a connect that a test drives: another `ssh` program.
    #[doc(hidden)]
    #[must_use]
    pub fn with_program(mut self, program: impl Into<String>) -> Self {
        self.program = program.into();
        self
    }

    /// The settings of a connect that a test drives: another bound for one
    /// forward.
    #[doc(hidden)]
    #[must_use]
    pub fn with_forward_deadline(mut self, deadline: Duration) -> Self {
        self.forward_deadline = deadline;
        self
    }

    /// The settings of a connect that a test drives: a readiness probe that does
    /// not wait for a real forward.
    #[doc(hidden)]
    #[must_use]
    pub fn with_probe(mut self, probe: impl Fn(u16) -> bool + Send + Sync + 'static) -> Self {
        self.probe = Arc::new(probe);
        self
    }
}
