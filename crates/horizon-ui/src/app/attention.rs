//! The attention state of each agent panel of the board.
//!
//! A launch that opted in to work continuation registers a work ledger, and the
//! lifecycle hooks of the agent keep that ledger current. The ledger files live
//! in the Horizon home, so one worker thread reads them: a frame reads the value
//! that the last read published, and the fallback of an agent kind without hooks
//! only reads the working indicator in memory.
//!
//! The state is made when a read applies, when the focus changes and when the
//! person marks a row unread. A read runs at most every [`INTERVAL`], and a read
//! that brings the same ledgers back wakes no frame: the worker asks for a
//! repaint only when its answer differs from the one before it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use horizon_core::agent_work::{
    ProgramStatus, TurnLedger, WorkStore, program_status_reports_state, title_reports_state,
};
use horizon_core::{Attention, AttentionBoard, AttentionTarget, PanelId, current_unix_millis};

use super::HorizonApp;

/// How often the work ledgers of the agent panels are read at most.
const INTERVAL: Duration = Duration::from_millis(500);

/// What the jump says when no agent panel waits for the person.
pub(in crate::app) const NOBODY_WAITS: &str = "No agent needs you.";

/// The ledgers of one read: the panel, its local id and the ledger of its
/// launch, when that launch registered one.
pub(in crate::app) type AttentionRead = Vec<(PanelId, String, Option<TurnLedger>)>;

/// The attention states of the agent panels, the reads that keep them and the
/// look of the person.
#[derive(Default)]
pub(in crate::app) struct AttentionState {
    states: AttentionBoard,
    worker: Option<AttentionWorker>,
    /// When the last read was asked for.
    read_at: Option<Instant>,
    /// Whether a read runs on the worker.
    in_flight: bool,
    /// The panel that had the keyboard focus at the last check.
    focused: Option<PanelId>,
    /// The context that a change of a state wakes, so a changed row repaints
    /// once and an unchanged one repaints nothing.
    wake: Option<egui::Context>,
    /// Counts the changes of a state, so a cached sidebar row knows that its
    /// badge is old.
    version: u64,
}

impl AttentionState {
    /// The states of the agent panels of `home`, read on one worker thread.
    pub(in crate::app) fn new(home: &Path) -> Self {
        Self {
            states: AttentionBoard::default(),
            worker: AttentionWorker::start(home),
            ..Self::default()
        }
    }

    /// The state of the agent panel whose local id is `panel`.
    pub(in crate::app) fn state(&self, panel: &str) -> Attention {
        self.states.state(panel)
    }

    /// When the turn of `panel` reached its state, so the panel that waited
    /// longest comes first.
    pub(in crate::app) fn since_millis(&self, panel: &str) -> i64 {
        self.states.since_millis(panel)
    }

    /// Drops the panels that are no longer on the board.
    fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.states.retain(keep);
    }

    /// The number of changes of the states so far.
    pub(in crate::app) fn version(&self) -> u64 {
        self.version
    }

    /// Whether a read of the ledgers runs on the worker now.
    pub(in crate::app) fn reads(&self) -> bool {
        self.in_flight
    }

    /// Counts one change of a state.
    fn count_change(&mut self) {
        self.version = self.version.wrapping_add(1);
    }
}

/// The worker thread that reads the work ledgers of the panels.
struct AttentionWorker {
    jobs: Sender<Vec<(PanelId, String)>>,
    reads: Receiver<AttentionRead>,
}

impl AttentionWorker {
    /// Starts the worker, or answers `None` when this process cannot start a
    /// thread. Without it no ledger is read and every agent panel keeps the
    /// signal of its terminal.
    fn start(home: &Path) -> Option<Self> {
        let store = WorkStore::new(home);
        let (jobs, received) = channel::<Vec<(PanelId, String)>>();
        let (reads, read) = channel::<AttentionRead>();
        let spawned = std::thread::Builder::new()
            .name("agent-attention".into())
            .spawn(move || read_ledgers(&store, received, &reads));
        match spawned {
            Ok(_) => Some(Self { jobs, reads: read }),
            Err(error) => {
                tracing::warn!(%error, "the attention read of the agent panels runs on no worker");
                None
            }
        }
    }

    /// Asks for one read. Answers whether the worker took the job.
    fn ask(&self, panels: Vec<(PanelId, String)>) -> bool {
        self.jobs.send(panels).is_ok()
    }

    /// The answer of a read, or `None` while the worker still reads. A worker
    /// that ended answers `Err`, so the state stops asking it.
    fn take(&self) -> Result<Option<AttentionRead>, ()> {
        match self.reads.try_recv() {
            Ok(read) => Ok(Some(read)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }
}

/// Reads the ledger of each panel, in the order the frame asked for it. The
/// answer wakes no frame of its own: the frame that asked reads it, and asks for
/// one repaint only when a state changed.
fn read_ledgers(store: &WorkStore, received: Receiver<Vec<(PanelId, String)>>, reads: &Sender<AttentionRead>) {
    for panels in received {
        let read: AttentionRead = panels
            .into_iter()
            .map(|(panel, local_id)| {
                // A record that this home cannot read changes nothing.
                let ledger = store.read_report(&local_id).ok().flatten().map(|record| record.ledger);
                (panel, local_id, ledger)
            })
            .collect();
        if reads.send(read).is_err() {
            return;
        }
    }
}

impl HorizonApp {
    /// Refreshes the attention states of the agent panels of this board.
    ///
    /// The fallback of an agent without hooks and the look of the person are
    /// values in memory, so they run on the frame that hears the focus change,
    /// on the frame that applies a read, and on the refresh tick. Only the tick
    /// asks the worker for a read, and only while no read runs.
    pub(in crate::app) fn refresh_attention(&mut self, ctx: &egui::Context) {
        if self.attention.wake.is_none() {
            self.attention.wake = Some(ctx.clone());
        }
        self.apply_attention_reads(ctx);
        // The title of an agent that reports its state there changes about ten
        // times a second, and only a change of the state is work: the claim of a
        // panel that holds its state wakes no frame.
        let (titled, moved) = self.observe_agent_claims(current_unix_millis());
        if titled {
            self.attention.count_change();
            ctx.request_repaint();
            self.note_changed_panels(moved, ctx);
        }
        let now = Instant::now();
        let focus_changed = self.attention.focused != self.board.focused;
        if focus_changed {
            self.attention.focused = self.board.focused;
            if let Some(local_id) = self.focused_panel_local_id()
                && self.attention.states.note_look(&local_id, current_unix_millis())
            {
                self.attention.count_change();
                ctx.request_repaint();
            }
        }
        let due = self
            .attention
            .read_at
            .is_none_or(|asked| now.duration_since(asked) >= INTERVAL);
        if focus_changed || due {
            let moved = self.observe_agent_indicators();
            if !moved.is_empty() {
                self.attention.count_change();
                ctx.request_repaint();
                self.note_changed_panels(moved, ctx);
            }
        }
        // The focus of the windows is the value that the frame already has, so the
        // notification of a change waits for no other read.
        self.notifications
            .decide(self.board.workspaces.len() > 1, Instant::now(), ctx);
        if !due || self.attention.in_flight {
            return;
        }
        let Some(worker) = &self.attention.worker else {
            return;
        };
        if !self.board.panels.iter().any(|panel| panel.kind.is_agent()) {
            return;
        }
        let panels = self.agent_panel_ids();
        self.attention.read_at = Some(now);
        self.attention.in_flight = worker.ask(panels);
    }

    /// The agent panels of the board, with the local id of the work ledger that
    /// its launch would have registered.
    fn agent_panel_ids(&self) -> Vec<(PanelId, String)> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.kind.is_agent())
            .map(|panel| (panel.id, panel.local_id.clone()))
            .collect()
    }

    /// Applies the read of the worker that came back. A worker that ended can
    /// answer no later read, so the state stops asking it.
    fn apply_attention_reads(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.attention.worker else {
            return;
        };
        let read = match worker.take() {
            Ok(Some(read)) => read,
            Ok(None) => return,
            Err(()) => {
                tracing::warn!("the attention worker ended; the agent panels keep the state they have");
                self.attention.worker = None;
                self.attention.in_flight = false;
                return;
            }
        };
        self.attention.in_flight = false;
        let _ = self.apply_attention_values(read, ctx);
    }

    /// Keeps the ledgers that came back: the state of a panel that has one comes
    /// from the hook events, and one repaint follows only a changed state.
    /// Answers whether the read asked for that repaint.
    pub(in crate::app) fn apply_attention_values(&mut self, read: AttentionRead, ctx: &egui::Context) -> bool {
        let now_millis = current_unix_millis();
        let focused = self.focused_panel_local_id();
        let mut changed = false;
        let mut moved = Vec::new();
        for (_, local_id, ledger) in read {
            let Some(ledger) = ledger else {
                continue;
            };
            if self.attention.states.observe_ledger(&local_id, &ledger, now_millis) {
                changed = true;
                moved.push(local_id.clone());
            }
            if focused.as_deref() == Some(local_id.as_str()) {
                // The person looks at the panel that the read describes.
                changed |= self.attention.states.note_look(&local_id, now_millis);
            }
        }
        if changed {
            self.attention.count_change();
            ctx.request_repaint();
        }
        // The state of a change that a look cleared is not a state that waits.
        self.note_changed_panels(moved, ctx);
        changed
    }

    /// Queues one desktop notification for each panel of `moved` that now waits
    /// for the person, while no window of Horizon has the focus.
    fn note_changed_panels(&mut self, moved: Vec<String>, ctx: &egui::Context) {
        if moved.is_empty() {
            return;
        }
        let panels: Vec<(String, Attention)> = moved
            .into_iter()
            .map(|local_id| {
                let state = self.attention.state(&local_id);
                (local_id, state)
            })
            .collect();
        self.note_attention_changes(panels, ctx);
    }

    /// Records the state that each agent panel claims: the reports of the program
    /// status protocol first, and the title of its terminal for the kinds that
    /// write their state there.
    ///
    /// The read is a value that the frame already holds: the panel keeps the newest
    /// report that the reader of its PTY handed over, and it compares the title of
    /// its terminal when that title changes. The protocol answers for a panel from
    /// the first report of its launch on, so the titles of a program that speaks it
    /// are not read. Neither claim does work for a report or a title that repeats
    /// the state that it already holds. Only a panel of a kind with one of the two
    /// sources is asked, so a board of other panels pays two comparisons for each
    /// of them.
    ///
    /// Answers whether a panel shows another state, and the panels whose state
    /// changed, so the caller wakes one frame and queues the notifications of the
    /// panels that wait.
    fn observe_agent_claims(&mut self, now_millis: i64) -> (bool, Vec<String>) {
        let focused = self.focused_panel_local_id();
        let mut changed = false;
        let mut moved = Vec::new();
        for panel in &self.board.panels {
            let reports = program_status_reports_state(panel.kind);
            if !reports && !title_reports_state(panel.kind) {
                continue;
            }
            let mut answered = false;
            if reports && panel.program_status() != ProgramStatus::Unknown {
                answered = self.attention.states.observe_program(
                    &panel.local_id,
                    panel.launched_at_millis,
                    panel.program_status(),
                    now_millis,
                );
            }
            // A panel whose program has written no title has made no statement
            // about itself. An empty title is not the ready title of a launch, so
            // it is not read at all.
            if title_reports_state(panel.kind) && !panel.terminal_title.is_empty() {
                answered |= self.attention.states.observe_title(
                    &panel.local_id,
                    panel.launched_at_millis,
                    panel.title_state(),
                    now_millis,
                );
            }
            if answered {
                changed = true;
                moved.push(panel.local_id.clone());
            }
            if focused.as_deref() == Some(panel.local_id.as_str()) {
                // The person looks at this panel, so a turn of it that ends is no
                // state that waits for them.
                changed |= self.attention.states.note_look(&panel.local_id, now_millis);
            }
        }
        (changed, moved)
    }

    /// Records the working indicator of each agent panel whose launch has no
    /// ledger, and drops the panels that left the board. Answers whether a panel
    /// shows another state now.
    fn observe_agent_indicators(&mut self) -> Vec<String> {
        let now_millis = current_unix_millis();
        let focused = self.focused_panel_local_id();
        let mut moved = Vec::new();
        for panel in &self.board.panels {
            if !panel.kind.is_agent() {
                continue;
            }
            let is_focused = focused.as_deref() == Some(panel.local_id.as_str());
            if self.attention.states.observe_working(
                &panel.local_id,
                panel.agent_status() == horizon_core::AgentStatus::Working,
                is_focused,
                now_millis,
            ) {
                moved.push(panel.local_id.clone());
            }
        }
        let local_ids: HashSet<&str> = self.board.panels.iter().map(|panel| panel.local_id.as_str()).collect();
        self.attention.retain(|local_id| local_ids.contains(local_id));
        moved
    }

    /// The local id of the panel that has the keyboard focus.
    fn focused_panel_local_id(&self) -> Option<String> {
        self.board
            .focused
            .and_then(|panel| self.board.panel(panel))
            .map(|panel| panel.local_id.clone())
    }

    /// Records that the person looked at an agent panel: they gave it the focus
    /// or clicked its row. Answers whether the panel shows another state now.
    pub(in crate::app) fn note_attention_look(&mut self, local_id: &str) -> bool {
        let changed = self.attention.states.note_look(local_id, current_unix_millis());
        if changed {
            self.attention.count_change();
            if let Some(ctx) = &self.attention.wake {
                ctx.request_repaint();
            }
        }
        changed
    }

    /// Records that the person clicked the row of `panel`: a click is a look at
    /// the panel, also while it already has the focus.
    pub(in crate::app) fn note_panel_attention_look(&mut self, panel: PanelId) {
        let Some(local_id) = self.board.panel(panel).map(|panel| panel.local_id.clone()) else {
            return;
        };
        self.note_attention_look(&local_id);
    }

    /// Marks an agent panel unread, which the menu of a panel row and of a panel
    /// titlebar offers on an idle agent.
    pub(in crate::app) fn mark_panel_unread(&mut self, panel: PanelId) {
        let Some(local_id) = self.board.panel(panel).map(|panel| panel.local_id.clone()) else {
            return;
        };
        self.mark_local_id_unread(&local_id);
    }

    fn mark_local_id_unread(&mut self, local_id: &str) {
        if !self.attention.states.mark_unread(local_id) {
            return;
        }
        self.attention.count_change();
        if let Some(ctx) = &self.attention.wake {
            ctx.request_repaint();
        }
    }

    /// The state of the agent panel whose local id is `local_id`.
    pub(in crate::app) fn panel_attention(&self, local_id: &str) -> Attention {
        self.attention.state(local_id)
    }

    /// The state of one panel of the board, whose program may have ended.
    ///
    /// An ended program runs nothing and waits for nothing, and the five states
    /// name the turns of an agent, so none of them fits it. Idle is the state
    /// that keeps the row quiet: the ledger of a panel that exited in the middle
    /// of a turn would otherwise say that the agent still works.
    pub(in crate::app) fn panel_attention_of(&self, panel: &horizon_core::Panel) -> Attention {
        if panel.program_end().is_some() {
            return Attention::Idle;
        }
        if let Some(card) = panel.block() {
            return self.card_attention(panel.id, card);
        }
        self.panel_attention(&panel.local_id)
    }

    /// The state of one card. A card asks the person while its document holds a
    /// callout with at least one button that the person has not pressed, so the
    /// row of the card says `Needs you` in the sidebar even when the card itself
    /// is outside the view. A press on an answer of that callout ends it, and so
    /// does a document without such a callout.
    ///
    /// The answer is saved with the card, so a restored card reads it here. The
    /// live card holds the press of this run, and the saved one holds the press of
    /// the last frame that settled, so the two are read together: a press that a
    /// frame has not yet written through ends the row at once.
    fn card_attention(&self, panel_id: PanelId, card: &horizon_core::BlockPanelState) -> Attention {
        let Some(callout) = card.document.components.iter().find(|component| match component {
            horizon_browser_control::block::Component::Callout { buttons, .. } => !buttons.is_empty(),
            _ => false,
        }) else {
            return Attention::Idle;
        };
        let answered = self
            .panel_render_caches
            .card_runtime
            .get(&panel_id)
            .is_some_and(|runtime| runtime.pressed_answer_of(callout))
            || card.pressed_answer_of(callout);
        if answered { Attention::Idle } else { Attention::NeedsYou }
    }

    /// The agent panels that a jump goes to: the panels that need the person,
    /// with the moment their turn reached that state.
    pub(in crate::app) fn attention_targets(&self) -> Vec<AttentionTarget> {
        self.board
            .panels
            .iter()
            .filter(|panel| panel.kind.is_agent())
            .filter_map(|panel| {
                let attention = self.attention.state(&panel.local_id);
                attention.waits().then_some(AttentionTarget {
                    panel: panel.id,
                    attention,
                    since_millis: self.attention.since_millis(&panel.local_id),
                })
            })
            .collect()
    }

    /// Goes to the agent panel that waits longest for the person: it switches
    /// the workspace, reveals the panel and gives it the focus, in the path of a
    /// click on its row. With no panel of this window waiting, it does what a press on
    /// the row of an agent of a machine does, and with no agent waiting it says so in
    /// one line.
    pub(in crate::app) fn go_to_next_attention(&mut self, ctx: &egui::Context) {
        match horizon_core::next_attention_target(&self.attention_targets(), self.board.focused) {
            Some(panel) => self.reveal_selected_panel(ctx, panel),
            None => match self.waiting_machine_agent() {
                Some(agent) => self.open_machine_agent(&agent, ctx),
                None => self.set_sidebar_plain_notice(NOBODY_WAITS.to_owned()),
            },
        }
    }

    /// The agent of a machine that the shortcut reaches: the first one that needs an
    /// answer, then the first one that finished unseen, in the order of the snapshot,
    /// which is the name of its machine and then its session and its pane.
    ///
    /// The panels of this window come first: they stand in front of the person, and the
    /// agents of a machine are the rows that no window shows.
    fn waiting_machine_agent(&self) -> Option<horizon_core::machine_watch::MachineAgent> {
        let snapshot = self.machine_watch.watch.snapshot();
        let mut waiting: Vec<&horizon_core::machine_watch::MachineAgent> = snapshot
            .machines
            .iter()
            .flat_map(|machine| &machine.agents)
            .filter(|agent| agent.state.attention().waits() && agent.can_open())
            .collect();
        waiting.sort_by_key(|agent| std::cmp::Reverse(agent.state.attention().rank()));
        waiting.first().map(|agent| (*agent).clone())
    }
}

#[cfg(test)]
mod codex_status_tests;
#[cfg(test)]
mod codex_title_tests;
#[cfg(test)]
mod tests;
