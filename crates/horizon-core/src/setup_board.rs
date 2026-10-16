//! The setup board: one panel that lists the repositories whose cloud setup agents work
//! on this computer. The app fills its rows each frame from the setup runs; the panel
//! only draws them and asks the app to act.

use crate::PanelId;

/// Panel content of the setup board.
#[derive(Debug, Default)]
pub struct SetupBoard {
    /// One row for each setup run, in the order the runs started.
    pub rows: Vec<Row>,
    /// What the person pressed while the board was drawn, for the app to carry out.
    pub request: Option<Request>,
    /// The rented builder, with what it costs, as one line, and whether the line tells of
    /// a failure; none while setups rent no builder.
    pub builder: Option<(String, bool)>,
}

/// One repository being set up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The agent panel of the run, which names the run.
    pub run: PanelId,
    /// The repository's folder name.
    pub name: String,
    /// The repository as the person entered it.
    pub path: String,
    /// The agent's preset name.
    pub agent: String,
    /// The Docker host its proof builds on, by name; none for this computer.
    pub build_on: Option<String>,
    /// The state of each setup step, in order.
    pub steps: Vec<Step>,
    /// What the run does now, as one short line.
    pub now: String,
    pub state: RowState,
    /// **Use this setup** commits the setup now; Stop waits until it reported.
    pub committing: bool,
    /// `now` is a message about something that did not happen, such as a refused Use.
    pub alert: bool,
    /// The agent's panel is still open, so Open agent can show it.
    pub agent_open: bool,
}

/// A step's state as a progress segment shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Pending,
    /// Waiting for a build slot that other setups hold.
    Waiting,
    Running,
    Done,
    Skipped,
    Failed,
}

/// Where a run is, as the row's line and its buttons follow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    /// The agent or Horizon's proof works.
    Working,
    /// The proof waits for a build slot.
    Waiting,
    /// The agent reported a summary: the setup can be used.
    Ready,
    /// A step failed; the agent can still fix it.
    Failed,
    /// The agent's panel closed before the setup was ready.
    Stopped,
}

/// What the person asked of the board.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Show the run's agent panel.
    OpenAgent(PanelId),
    /// Open New cloud for the run's repository, which shows its progress and Use.
    OpenInNewCloud(PanelId),
    /// Stop the run, or take a stopped run off the board.
    Stop(PanelId),
    /// Open New cloud to set up another repository.
    NewSetup,
}

impl SetupBoard {
    /// How many rows are in each state that counts as busy: working and waiting.
    #[must_use]
    pub fn busy(&self) -> (usize, usize) {
        let working = self.rows.iter().filter(|row| row.state == RowState::Working).count();
        let waiting = self.rows.iter().filter(|row| row.state == RowState::Waiting).count();
        (working, waiting)
    }
}
