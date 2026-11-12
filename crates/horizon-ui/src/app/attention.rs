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
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use horizon_core::agent_work::{TurnLedger, WorkStore};
use horizon_core::{Attention, AttentionBoard, PanelId, current_unix_millis};

use super::HorizonApp;

/// How often the work ledgers of the agent panels are read at most.
const INTERVAL: Duration = Duration::from_millis(500);

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

    /// Drops the panels that are no longer on the board.
    fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.states.retain(keep);
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

    fn ask(&self, panels: Vec<(PanelId, String)>) {
        let _ = self.jobs.send(panels);
    }

    fn take(&self) -> Option<AttentionRead> {
        // No read yet, and a worker that ended, both leave the state as it is.
        self.reads.try_recv().ok()
    }
}

/// Reads the ledger of each panel, in the order the frame asked for it, and
/// wakes the window only when an answer differs from the one before it.
fn read_ledgers(store: &WorkStore, received: Receiver<Vec<(PanelId, String)>>, reads: &Sender<AttentionRead>) {
    let mut last: Option<AttentionRead> = None;
    for panels in received {
        let read: AttentionRead = panels
            .into_iter()
            .map(|(panel, local_id)| {
                // A record that this home cannot read changes nothing.
                let ledger = store.read(&local_id).ok().flatten().map(|record| record.ledger);
                (panel, local_id, ledger)
            })
            .collect();
        if last.as_ref() == Some(&read) {
            continue;
        }
        last = Some(read.clone());
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
        let now = Instant::now();
        let focus_changed = self.attention.focused != self.board.focused;
        if focus_changed {
            self.attention.focused = self.board.focused;
            if let Some(local_id) = self.focused_panel_local_id()
                && self.attention.states.note_look(&local_id, current_unix_millis())
            {
                ctx.request_repaint();
            }
        }
        let due = self
            .attention
            .read_at
            .is_none_or(|asked| now.duration_since(asked) >= INTERVAL);
        if focus_changed || due {
            self.observe_agent_indicators();
        }
        if !due || self.attention.in_flight {
            return;
        }
        let panels = self.agent_panel_ids();
        if panels.is_empty() {
            return;
        }
        self.attention.read_at = Some(now);
        self.attention.in_flight = true;
        if let Some(worker) = &self.attention.worker {
            worker.ask(panels);
        }
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

    /// Applies the read of the worker that came back.
    fn apply_attention_reads(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.attention.worker else {
            return;
        };
        let Some(read) = worker.take() else {
            return;
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
        for (_, local_id, ledger) in read {
            let Some(ledger) = ledger else {
                continue;
            };
            changed |= self.attention.states.observe_ledger(&local_id, &ledger, now_millis);
            if focused.as_deref() == Some(local_id.as_str()) {
                // The person looks at the panel that the read describes.
                changed |= self.attention.states.note_look(&local_id, now_millis);
            }
        }
        if changed {
            ctx.request_repaint();
        }
        changed
    }

    /// Records the working indicator of each agent panel whose launch has no
    /// ledger, and drops the panels that left the board.
    fn observe_agent_indicators(&mut self) {
        let now_millis = current_unix_millis();
        let focused = self.focused_panel_local_id();
        for panel in &self.board.panels {
            if !panel.kind.is_agent() {
                continue;
            }
            let is_focused = focused.as_deref() == Some(panel.local_id.as_str());
            self.attention.states.observe_working(
                &panel.local_id,
                panel.agent_status() == horizon_core::AgentStatus::Working,
                is_focused,
                now_millis,
            );
        }
        let local_ids: HashSet<&str> = self.board.panels.iter().map(|panel| panel.local_id.as_str()).collect();
        self.attention.retain(|local_id| local_ids.contains(local_id));
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
        if changed && let Some(ctx) = &self.attention.wake {
            ctx.request_repaint();
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
        if self.attention.states.mark_unread(local_id)
            && let Some(ctx) = &self.attention.wake
        {
            ctx.request_repaint();
        }
    }

    /// The state of the agent panel whose local id is `local_id`.
    pub(in crate::app) fn panel_attention(&self, local_id: &str) -> Attention {
        self.attention.state(local_id)
    }
}

#[cfg(test)]
mod tests;
