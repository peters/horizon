//! Horizon coordination around the portable cloud provider. No work happens on the UI thread.
pub use horizon_cloud::{Cancellation, CreateState};
pub mod browser_auth;
pub mod command;
pub mod deployment;
pub mod git_auth;
pub mod image;
pub mod lifecycle;
pub mod progress;
pub mod repository;
pub mod settings;
pub mod ssh;
pub mod state;
pub mod tunnel;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("Cloud state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid cloud state or settings")]
    Json,
    #[error(transparent)]
    Provider(#[from] horizon_cloud::CloudError),
    #[error("{0} failed; inspect deployment output")]
    Command(&'static str),
    #[error("Another controller owns this cloud operation")]
    Busy,
}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Debug)]
pub enum Event {
    Stage(Stage, std::time::Instant),
    Progress(progress::Progress),
    Output(String),
    Ready(Box<state::Deployment>, std::time::Instant),
    Snapshot(Box<state::Deployment>),
    Failed(String, std::time::Instant),
    Desktop(std::sync::Arc<tunnel::DesktopTunnel>),
    Browsers(Vec<horizon_browser_protocol::cloud_view::CloudViewState>),
    Deleted,
    Stopped(Box<state::Deployment>),
    Resumed,
    ClosedBrowsers(Vec<String>),
    DesktopControl {
        active: Option<String>,
        last: Option<String>,
    },
}
impl Event {
    #[must_use]
    pub fn stage(stage: Stage) -> Self {
        Self::Stage(stage, std::time::Instant::now())
    }
    #[must_use]
    pub fn ready(state: Box<state::Deployment>) -> Self {
        Self::Ready(state, std::time::Instant::now())
    }
    #[must_use]
    pub fn failed(error: String) -> Self {
        Self::Failed(error, std::time::Instant::now())
    }
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum Stage {
    Validate,
    Build,
    Push,
    Provision,
    Readiness,
    Worktrees,
    Sessions,
    Ready,
    Stopped,
    Stopping,
    Deleted,
}
impl Stage {
    pub const ALL: [Self; 8] = [
        Self::Validate,
        Self::Build,
        Self::Push,
        Self::Provision,
        Self::Readiness,
        Self::Worktrees,
        Self::Sessions,
        Self::Ready,
    ];
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Validate => "Validate",
            Self::Build => "Build locally",
            Self::Push => "Push image",
            Self::Provision => "Provision worker",
            Self::Readiness => "Check readiness",
            Self::Worktrees => "Prepare worktrees",
            Self::Sessions => "Start sessions",
            Self::Ready => "Ready",
            Self::Stopped => "Stopped",
            Self::Stopping => "Stop requested",
            Self::Deleted => "Worker deleted",
        }
    }
}

#[must_use]
pub fn new_id() -> String {
    crate::runtime_state::new_local_id()
}
