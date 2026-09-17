//! Conservative evidence for continuing work after a host restart.
//! Reading a conversation is not permission to start another turn.

mod command;
mod discovery;
mod ledger;
mod lifecycle;
mod policy;
mod repository;
mod startup;
mod store;
mod transcript;

pub use startup::resume_brief;
pub(crate) use startup::{RestoreBudget, WorkContinuation};

pub(crate) use lifecycle::{WorkLaunch, WorkOwner};

pub use ledger::{HookEvent, TurnLedger};
pub use policy::{AskReason, RestartDecision, RestartEvidence, ResumePolicy, SuspendRecord};
pub use store::{HookInput, StoredWork, WorkStore};
pub use transcript::{TranscriptSnapshot, TurnState};

pub const WORK_ROOT_ENV: &str = "HORIZON_WORK_ROOT";
pub const WORK_PANEL_ENV: &str = "HORIZON_WORK_PANEL";
pub const WORK_OWNER_ENV: &str = "HORIZON_WORK_OWNER";
pub const WORK_KIND_ENV: &str = "HORIZON_WORK_KIND";
pub const WORK_EXECUTABLE_ENV: &str = "HORIZON_WORK_EXECUTABLE";
#[cfg(not(windows))]
pub const HOOK_COMMAND: &str = "\"${HORIZON_WORK_EXECUTABLE}\" --agent-work-hook";

#[cfg(windows)]
pub const HOOK_COMMAND: &str = "& \"$env:HORIZON_WORK_EXECUTABLE\" --agent-work-hook";
pub const HOOK_SHELL: &str = if cfg!(windows) { "powershell" } else { "bash" };
pub(crate) const WORK_ENV_KEYS: [&str; 5] = [
    WORK_ROOT_ENV,
    WORK_PANEL_ENV,
    WORK_OWNER_ENV,
    WORK_KIND_ENV,
    WORK_EXECUTABLE_ENV,
];
