//! What the app keeps about Upkeeps between frames.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use horizon_core::WorkspaceId;
use horizon_core::upkeep::worker::{Config, Signals};

use super::form::Form;

/// The saved Upkeeps: cloud id and choice. Shared, so a frame reads them without a copy.
pub(in crate::app) type Saved = Arc<Vec<(String, Config)>>;

/// A read of the saved Upkeeps, with the clouds whose first run was not sent yet and its token.
pub(in crate::app) type Read = (Saved, Vec<(String, u128)>);

/// The Upkeeps of this computer: the dialog, a start under way, and the exchange with
/// each Upkeep cloud's worker.
#[derive(Default)]
pub(in crate::app) struct State {
    pub(in crate::app) form: Option<Form>,
    pub(in crate::app) preparing: Option<Preparing>,
    /// The saved Upkeeps, once read; `None` reads them again.
    pub(in crate::app) saved: Option<Saved>,
    /// The read of the saved Upkeeps under way.
    pub(in crate::app) reading: Option<Receiver<Read>>,
    pub(in crate::app) syncs: HashMap<String, Sync>,
}

/// A start waiting for its first repository on this computer.
pub(in crate::app) struct Preparing {
    pub(in crate::app) receiver: Receiver<Result<Prepared, String>>,
    pub(in crate::app) cloud_id: String,
    /// The session the start began in: the cloud is recorded with it or not at all.
    pub(in crate::app) session: Option<String>,
    /// Where the cloud goes; `None` makes a new workspace.
    pub(in crate::app) workspace: Option<WorkspaceId>,
}

/// The first repository on this computer, at the commit the cloud starts from.
pub(in crate::app) struct Prepared {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) revision: String,
    /// The token of the first Run now, which the first-run marker keeps too.
    pub(in crate::app) run_token: u128,
}

/// What one exchange with an Upkeep worker learned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct Answer {
    /// The loop runs and ran before this exchange, so a panel can attach to it.
    pub(in crate::app) settled: bool,
    /// A run is in progress.
    pub(in crate::app) running: bool,
}

/// What an exchange hands back: the GitHub access requests it allowed, also when the rest
/// of it failed, and what it learned.
pub(in crate::app) type Exchanged = (Vec<String>, Result<Answer, String>);

/// One Upkeep cloud's exchange with its worker.
#[derive(Default)]
pub(in crate::app) struct Sync {
    pub(in crate::app) inflight: Option<Receiver<Exchanged>>,
    /// The requests this Upkeep allowed, newest last, which the cloud card no longer offers.
    pub(in crate::app) allowed: Vec<String>,
    /// What the exchange under way sent, put back if it fails.
    pub(in crate::app) sent: Signals,
    pub(in crate::app) next: Option<Instant>,
    /// What the board asked for since the last exchange.
    pub(in crate::app) signals: Signals,
    pub(in crate::app) last: Answer,
    /// The live panel was opened in this run of Horizon: one the person closes stays closed.
    pub(in crate::app) agent_shown: bool,
}
