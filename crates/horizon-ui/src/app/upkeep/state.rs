//! What the app keeps about Upkeeps between frames.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use horizon_core::upkeep::native::{Approved, Ledger, Report, Request, probe};
use horizon_core::upkeep::worker::{Config, Signals};
use horizon_core::{PanelId, WorkspaceId};

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
    /// The Repositories editor of a board.
    pub(in crate::app) editor: Option<super::editor::Editor>,
    pub(in crate::app) preparing: Option<Preparing>,
    /// The saved Upkeeps, once read; `None` reads them again.
    pub(in crate::app) saved: Option<Saved>,
    /// The read of the saved Upkeeps under way.
    pub(in crate::app) reading: Option<Receiver<Result<Read, String>>>,
    /// After a failed read, when to read again.
    pub(in crate::app) read_after: Option<Instant>,
    /// Reads that failed in a row; after a few, a choice that cannot be read is left out
    /// rather than holding back every Upkeep.
    pub(in crate::app) read_failures: u32,
    pub(in crate::app) syncs: HashMap<String, Sync>,
    /// The `cloud_upkeep` requests of agents.
    pub(in crate::app) agent: super::agent::Requests,
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
    /// The loop's session runs: `None` until an exchange answered, and after one failed.
    pub(in crate::app) loop_running: Option<bool>,
    /// The loop runs and ran before this exchange, so a panel can attach to it.
    pub(in crate::app) settled: bool,
    /// A run is in progress.
    pub(in crate::app) running: bool,
    /// The first-run marker could not be removed after the worker got the first run.
    pub(in crate::app) marker_left: bool,
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
    /// The first-run marker of a run already delivered is still to be removed.
    pub(in crate::app) marker_left: bool,
    pub(in crate::app) next: Option<Instant>,
    /// What the board asked for since the last exchange.
    pub(in crate::app) signals: Signals,
    pub(in crate::app) last: Answer,
    /// The live panel was opened in this run of Horizon: one the person closes stays closed.
    pub(in crate::app) agent_shown: bool,
    /// The native tests of this Upkeep.
    pub(in crate::app) native: Native,
}

impl Sync {
    /// Whether the last exchange found the loop running; `None` when no exchange answered.
    pub(in crate::app) fn loop_running(&self) -> Option<bool> {
        self.last.loop_running
    }
}

/// The native tests of one Upkeep, as the app follows them: see
/// [`horizon_core::upkeep::native`].
#[derive(Default)]
pub(in crate::app) struct Native {
    /// The saved ledger, once read.
    pub(in crate::app) ledger: Option<Ledger>,
    pub(in crate::app) loading: Option<Receiver<std::io::Result<Ledger>>>,
    /// After a failed read of the ledger, when to read it again.
    pub(in crate::app) load_after: Option<Instant>,
    /// The ledger changed since it was last saved.
    pub(in crate::app) dirty: bool,
    /// The save under way; it answers whether the ledger reached the disk.
    pub(in crate::app) saving: Option<Receiver<bool>>,
    /// The last save reached the disk.
    pub(in crate::app) stored: bool,
    /// After a failed save of the ledger, when to save it again.
    pub(in crate::app) save_after: Option<Instant>,
    /// Requests that exchanges read, waiting for the ledger.
    pub(in crate::app) intake: Option<Intake>,
    /// The check of the request that starts now.
    pub(in crate::app) approving: Option<Receiver<Result<(Approved, String), String>>>,
    /// The approved test and its cloud id, which starts once a saved ledger names the cloud,
    /// so that a restart finds the cloud.
    pub(in crate::app) launching: Option<(Approved, String)>,
    /// The agent panel on the test cloud, and when it opened.
    pub(in crate::app) agent: Option<(PanelId, Instant)>,
    /// After a failed try to open the agent panel, when to try again.
    pub(in crate::app) agent_after: Option<Instant>,
    /// The report of a test that ended, kept until its cloud can be closed: a cloud that
    /// deploys or saves cannot close yet.
    pub(in crate::app) ending: Option<Report>,
    /// The desktop viewer of the test cloud opened.
    pub(in crate::app) desktop_shown: bool,
    /// After a failed try to open the desktop viewer, when to try again.
    pub(in crate::app) desktop_after: Option<Instant>,
    /// When the task's Enter goes to the agent panel.
    pub(in crate::app) submit_at: Option<Instant>,
    pub(in crate::app) probing: Option<Receiver<Result<Probed, String>>>,
    /// The look under way is the last one, at the time limit, which copies the evidence.
    pub(in crate::app) last_look: bool,
    pub(in crate::app) next_probe: Option<Instant>,
    /// The test clouds whose close this run of Horizon started, so it starts each one once.
    pub(in crate::app) close_tried: std::collections::HashSet<String>,
    /// The ledger changed since the worker last got its reports.
    pub(in crate::app) undelivered: bool,
    pub(in crate::app) delivering: Option<Receiver<Result<(), String>>>,
    pub(in crate::app) next_delivery: Option<Instant>,
}

impl Native {
    /// Forgets the panels and the looks of the test that ran: after it ended, or when a
    /// session switch gives the panels new ids.
    pub(in crate::app) fn forget_test(&mut self) {
        self.launching = None;
        self.agent = None;
        self.agent_after = None;
        self.desktop_shown = false;
        self.desktop_after = None;
        self.submit_at = None;
        self.next_probe = None;
        self.probing = None;
        self.last_look = false;
    }
}

/// The channel from the exchanges to the native tests: the requests each exchange read.
pub(in crate::app) type Intake = (Sender<Vec<Request>>, Receiver<Vec<Request>>);

/// What one look at the test cloud found: the outcome, once its agent wrote one, with the
/// report text and the evidence files that Horizon kept, which a look before the outcome and
/// before the time limit does not copy.
pub(in crate::app) type Probed = (Option<probe::Outcome>, String, Vec<String>);
