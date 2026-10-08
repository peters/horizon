//! The three steps before the portfolio opens: connect GitHub, choose repositories,
//! and reach a worker. A worker step never unlocks before GitHub is connected, unless
//! the worker is a test worker that says GitHub is simulated.

use serde_json::Value;

/// What the Dependencies panel knows about its worker this frame.
#[derive(Clone, Copy, Debug)]
pub enum WorkerProbe<'a> {
    /// No worker can be reached from this machine yet.
    Unavailable,
    Connecting,
    Unreachable(&'a str),
    Reporting(&'a Value),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitHubStep {
    Connected,
    /// A test worker simulates GitHub; no account is used.
    Simulated,
    Needed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoriesStep {
    /// Waits for GitHub.
    Blocked,
    /// Chosen on GitHub; the worker confirms them once it reports.
    Choose,
    Reported {
        count: usize,
        simulated: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerStep {
    /// Waits for GitHub.
    Blocked,
    Unavailable,
    Connecting,
    Unreachable(String),
    Running {
        simulated: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub github: GitHubStep,
    pub repositories: RepositoriesStep,
    pub worker: WorkerStep,
}

impl Setup {
    #[must_use]
    pub fn evaluate(github_connected: bool, worker: WorkerProbe<'_>) -> Self {
        let report = match worker {
            WorkerProbe::Reporting(status) => Some(status),
            _ => None,
        };
        let simulated = report.is_some_and(|status| status["synthetic"] == true);
        let github = if github_connected {
            GitHubStep::Connected
        } else if simulated {
            GitHubStep::Simulated
        } else {
            GitHubStep::Needed
        };
        if github == GitHubStep::Needed {
            return Self {
                github,
                repositories: RepositoriesStep::Blocked,
                worker: WorkerStep::Blocked,
            };
        }
        let repositories = report.map_or(RepositoriesStep::Choose, |status| RepositoriesStep::Reported {
            count: status["repos"].as_array().map_or(0, Vec::len),
            simulated,
        });
        let worker = match worker {
            WorkerProbe::Unavailable => WorkerStep::Unavailable,
            WorkerProbe::Connecting => WorkerStep::Connecting,
            WorkerProbe::Unreachable(error) => WorkerStep::Unreachable(error.to_owned()),
            WorkerProbe::Reporting(_) => WorkerStep::Running { simulated },
        };
        Self {
            github,
            repositories,
            worker,
        }
    }

    /// The portfolio opens once GitHub is settled and a worker reports.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.github != GitHubStep::Needed && matches!(self.worker, WorkerStep::Running { .. })
    }

    /// The first step that still needs the person, counted from one.
    #[must_use]
    pub fn current(&self) -> usize {
        if self.github == GitHubStep::Needed {
            1
        } else if self.repositories == RepositoriesStep::Choose {
            2
        } else {
            3
        }
    }
}
