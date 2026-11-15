//! Conservative evidence for continuing work after a host restart.
//! Reading a conversation is not permission to start another turn.

mod command;
mod discovery;
mod ledger;
mod lifecycle;
mod policy;
mod program_status;
mod repository;
mod startup;
mod store;
mod title;
mod transcript;

pub use startup::resume_brief;
pub(crate) use startup::{RestoreBudget, WorkContinuation, append_seed, configured_resume_limit};

pub(crate) use lifecycle::{WorkLaunch, WorkOwner};

pub use ledger::{HookEvent, TurnLedger};
pub use policy::{AskReason, RestartDecision, RestartEvidence, ResumePolicy, SuspendRecord};
pub use program_status::{
    DETECTION_REPLY, MAX_REPORT_BYTES, ProgramClaim, ProgramStatus, program_status, program_status_reports_state,
};
pub use store::{HookInput, StoredWork, WorkStore};
pub use title::{MAX_STATE_TITLE_BYTES, TITLE_SETTLE_MILLIS, TitleClaim, TitleState, title_reports_state, title_state};
pub use transcript::{TranscriptSnapshot, TurnState};

use crate::horizon_home::HorizonHome;

pub const WORK_ROOT_ENV: &str = "HORIZON_WORK_ROOT";
pub const WORK_PANEL_ENV: &str = "HORIZON_WORK_PANEL";
pub const WORK_OWNER_ENV: &str = "HORIZON_WORK_OWNER";
pub const WORK_KIND_ENV: &str = "HORIZON_WORK_KIND";
pub const WORK_EXECUTABLE_ENV: &str = "HORIZON_WORK_EXECUTABLE";
pub const HOOK_COMMAND: &str = "\"${HORIZON_WORK_EXECUTABLE}\" --agent-work-hook";
pub const HOOK_SHELL: &str = "bash";
/// The argument of this binary that records one lifecycle event. A hook command
/// names it after the executable, and the report extension of a pi panel starts
/// the executable with it.
pub const HOOK_ARGUMENT: &str = "--agent-work-hook";
/// The values of [`WORK_KIND_ENV`]: the program whose events the record holds.
pub const WORK_KIND_CLAUDE: &str = "claude";
pub const WORK_KIND_CODEX: &str = "codex";
pub const WORK_KIND_PI: &str = "pi";

/// The name of a program that a launch of `kind` reports its state with.
#[must_use]
pub const fn work_kind_name(kind: crate::PanelKind) -> Option<&'static str> {
    match kind {
        crate::PanelKind::Claude => Some(WORK_KIND_CLAUDE),
        crate::PanelKind::Codex => Some(WORK_KIND_CODEX),
        crate::PanelKind::Pi => Some(WORK_KIND_PI),
        _ => None,
    }
}

/// The panel kind that a value of [`WORK_KIND_ENV`] names.
#[must_use]
pub fn work_kind_of(name: &str) -> Option<crate::PanelKind> {
    crate::agents::all_agent_kinds()
        .iter()
        .copied()
        .find(|kind| work_kind_name(*kind) == Some(name))
}

pub(crate) const WORK_ENV_KEYS: [&str; 5] = [
    WORK_ROOT_ENV,
    WORK_PANEL_ENV,
    WORK_OWNER_ENV,
    WORK_KIND_ENV,
    WORK_EXECUTABLE_ENV,
];

pub(crate) use startup::StartupPlan;

/// Removes the work record of a panel that left the board.
///
/// A closed panel continues nothing and reports nothing, so its launch keeps no
/// evidence in the home. The delete runs on a worker thread, because the caller
/// is a frame and no frame waits for a file.
pub(crate) fn forget_closed_panel(panel_local_id: &str) {
    if panel_local_id.is_empty() {
        return;
    }
    let panel = panel_local_id.to_owned();
    let spawned = std::thread::Builder::new()
        .name("agent-work-forget".into())
        .spawn(move || {
            let store = WorkStore::new(HorizonHome::resolve().root());
            if let Err(error) = store.forget(&panel) {
                tracing::debug!(%error, "the work record of a closed panel stays");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the work record of a closed panel stays");
    }
}
