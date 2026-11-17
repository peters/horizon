//! The list of the terminal panels of this computer, which a frame builds: it
//! needs the board, so it is the one part of the share that waits for a frame.
//!
//! The list is built again only after a trigger: terminal output on the frame
//! (a title and a state of a program arrive as output), another count of panels
//! or workspaces, a new answer of the attention reader, a change of the two
//! sharing switches, and one pass every two seconds, which catches a rename and
//! a resize that printed nothing. It is built at most four times a second.

use std::time::{Duration, Instant};

use egui::Context;
use horizon_core::{Attention, PanelKind};
use horizon_sync::{LiveComputer, LivePanel, MAX_LIVE_PANELS};

use super::super::HorizonApp;
use super::pump::OwnPanel;

/// The shortest time between two builds of the list.
const LIST_EVERY: Duration = Duration::from_millis(250);
/// The longest time without a build of the list.
const LIST_CHECK: Duration = Duration::from_secs(2);

/// What a list was built from: the counts of panels and workspaces, the version
/// of the attention states and the two sharing switches.
type ListKey = (usize, usize, u64, bool, bool);

/// When the list was built last, and from what.
#[derive(Default)]
pub(super) struct ListState {
    key: Option<ListKey>,
    pub(super) dirty: bool,
    built_at: Option<Instant>,
}

/// The states of a panel as the list names them. An idle panel names none.
const STATES: [(Attention, &str); 4] = [
    (Attention::Working, "working"),
    (Attention::Background, "background"),
    (Attention::Done, "done"),
    (Attention::NeedsYou, "blocked"),
];

/// The kind of a panel as the list names it.
fn kind_code(kind: PanelKind) -> String {
    let code = serde_json::to_value(kind).unwrap_or_default();
    code.as_str().unwrap_or_default().to_owned()
}

/// The kind that a list names, for the keyboard protocol. No terminal kind reads as a shell.
pub(in crate::app) fn kind_of(code: &str) -> PanelKind {
    let kind = serde_json::from_value(serde_json::Value::String(code.to_owned())).unwrap_or_default();
    match kind {
        PanelKind::Editor | PanelKind::Browser | PanelKind::Device | PanelKind::Block => PanelKind::Shell,
        kind => kind,
    }
}

/// The state that a list names, for the mark of a row.
pub(in crate::app) fn attention_of(code: &str) -> Attention {
    let named = STATES.iter().find(|(_, name)| *name == code);
    named.map_or(Attention::Idle, |(state, _)| *state)
}

/// The name of a computer for the person: its label, or its device name.
pub(in crate::app) fn computer_name(computer: &LiveComputer) -> &str {
    let label = Some(computer.label.as_str()).filter(|label| !label.is_empty());
    label.unwrap_or(&computer.device)
}

impl HorizonApp {
    /// The terminal panels of this computer, each with the source of its
    /// terminal: every panel with a terminal of its own, and no panel that
    /// follows another one. The pump gives the link none of them while this
    /// computer shares nothing.
    fn shared_list(&self) -> Vec<OwnPanel> {
        let server = &self.template_config.server;
        let listed = self.board.panels.iter().filter_map(|panel| {
            let terminal = panel.terminal()?;
            let source = terminal.share_source()?;
            let workspace = self.board.workspace(panel.workspace_id);
            let attention = self.panel_attention_of(panel);
            let state = STATES.iter().find(|(state, _)| *state == attention);
            let entry = LivePanel {
                id: panel.local_id.clone(),
                title: panel.display_title().into_owned(),
                kind: kind_code(panel.kind),
                state: state.map(|(_, name)| (*name).to_owned()).unwrap_or_default(),
                workspace: workspace.map(|workspace| workspace.name.clone()).unwrap_or_default(),
                cols: terminal.cols(),
                rows: terminal.rows(),
                typing: server.remote_typing,
            };
            Some(OwnPanel { entry, source })
        });
        listed.take(MAX_LIVE_PANELS).collect()
    }

    /// The list of this computer, when a trigger asked for a new build and the
    /// last build is old enough.
    pub(super) fn build_shared_list(&mut self, ctx: &Context, now: Instant) -> Option<Vec<OwnPanel>> {
        let server = &self.template_config.server;
        let key: ListKey = (
            self.board.panels.len(),
            self.board.workspaces.len(),
            self.attention.version(),
            server.share_terminals,
            server.remote_typing,
        );
        let list = &self.shared_terminals.list;
        let waited = list
            .built_at
            .map_or(Duration::MAX, |at| now.saturating_duration_since(at));
        if !list.dirty && list.key == Some(key) && waited < LIST_CHECK {
            return None;
        }
        if waited < LIST_EVERY {
            ctx.request_repaint_after(LIST_EVERY.saturating_sub(waited));
            return None;
        }
        let list = &mut self.shared_terminals.list;
        (list.dirty, list.key, list.built_at) = (false, Some(key), Some(now));
        Some(self.shared_list())
    }
}
