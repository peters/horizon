//! Conservative evidence for continuing work after a host restart.
//! Reading a conversation is not permission to start another turn.

mod ledger;
mod policy;
mod transcript;

pub use ledger::{HookEvent, TurnLedger};
pub use policy::{AskReason, RestartDecision, RestartEvidence, ResumePolicy, SuspendRecord};
pub use transcript::{TranscriptSnapshot, TurnState};
