//! The rows of the sidebar's workspace list. A row is read from the panels of its
//! workspace, so the rows are read again at most once a second, or at once when
//! workspaces come or go or a state of attention changes.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use horizon_core::Attention;
use horizon_core::WorkspaceId;
use horizon_core::workspace_list::{Orchestrator, Row, Turn, TurnEnd, Waiting};

use super::super::HorizonApp;
use super::inbox::Question;
use super::{SidebarActions, SidebarWorkspaceDragState, WorkspaceSidebarEntry, caption};

const REFRESH: Duration = Duration::from_secs(1);

/// The rows of the last read, by workspace, with the questions of its cards and
/// the turns of its orchestrator.
#[derive(Default)]
pub(in crate::app) struct ListCache {
    pub(super) refreshed: Option<Instant>,
    /// The id, persistent local id, name and machine of each workspace: ids
    /// restart at 1 in a replaced session, so the id alone does not tell the
    /// workspaces apart, and a workspace that moves to a machine or takes another
    /// name says so on the next frame.
    workspaces: Vec<(WorkspaceId, String, String, Option<String>)>,
    /// The version of the attention states that the rows came from, so a changed
    /// badge shows on the frame that changed it and not a second later.
    attention_version: u64,
    /// The changes that the questions and the words of the rows depend on, and
    /// the value that the last read saw.
    inbox_epoch: u64,
    read_epoch: u64,
    rows: HashMap<WorkspaceId, Row>,
    /// The questions of the cards of the board, the oldest first.
    pub(super) questions: Vec<Question>,
    /// The turn of the orchestrator of each workspace, by its local id, from the
    /// last read.
    turns: HashMap<String, TurnWatch>,
}

/// What one read saw of the orchestrator of a workspace: whether a turn runs,
/// how many turns failed, and the end of a turn that the person did not see in
/// the dock.
#[derive(Default)]
struct TurnWatch {
    busy: bool,
    failed: usize,
    unseen: Option<TurnEnd>,
}

impl TurnWatch {
    /// Keeps the turn that came back, and names the end of one that ended while
    /// the person looked elsewhere. A conversation that the dock shows is seen, so
    /// its end is no news: `shown` is true while the dock draws it.
    fn observe(&mut self, turn: Turn, shown: bool) {
        if self.busy && !turn.busy {
            self.unseen = Some(if turn.failed > self.failed {
                TurnEnd::Failed
            } else {
                TurnEnd::Done
            });
        }
        if shown {
            self.unseen = None;
        }
        self.busy = turn.busy;
        self.failed = turn.failed;
    }

    /// The turn of the orchestrator as the last read saw it.
    fn turn(&self) -> Turn {
        Turn {
            busy: self.busy,
            failed: self.failed,
        }
    }

    /// The end of a turn of the orchestrator that the person did not see.
    fn unseen(&self) -> Option<TurnEnd> {
        self.unseen
    }
}

impl ListCache {
    /// Counts a change that the questions and the words of the rows depend on: a
    /// new card, an update of a card, a press on a button of a card or a closed
    /// card. The next frame reads them again, also inside the second of a read.
    pub(in crate::app) fn count_inbox_change(&mut self) {
        self.inbox_epoch = self.inbox_epoch.wrapping_add(1);
    }
}

impl HorizonApp {
    /// The state of the orchestrator of a workspace, in the order of weight of one
    /// word: the question of a card, the failed turn, the working turn and the end
    /// of a turn that the person did not see. The read of the work items of the
    /// front desk uses the same rule as the row of the sidebar, so the two cannot
    /// disagree.
    pub(in crate::app) fn workspace_orchestrator(&self, workspace: WorkspaceId) -> Orchestrator {
        let asked = self
            .sidebar_list
            .questions
            .iter()
            .filter(|question| question.workspace == workspace)
            .count();
        self.workspace_orchestrator_with(workspace, asked)
    }

    /// The same state, from a count of the questions that a reader of the same
    /// moment holds, so a read that builds its own questions and its own state
    /// cannot disagree with itself.
    pub(in crate::app) fn workspace_orchestrator_with(&self, workspace: WorkspaceId, asked: usize) -> Orchestrator {
        let local_id = self
            .board
            .workspace(workspace)
            .map_or("", |workspace| workspace.local_id.as_str());
        let watch = self.sidebar_list.turns.get(local_id);
        Orchestrator::of(
            asked,
            watch.map_or_else(Turn::default, TurnWatch::turn),
            watch.and_then(TurnWatch::unseen),
        )
    }
}

impl HorizonApp {
    /// Reads the rows again when they are a second old, the workspaces changed,
    /// a state of attention changed or a card of the board changed.
    pub(in crate::app) fn refresh_sidebar_rows(&mut self, now: Instant) {
        let attention_version = self.attention.version();
        let cache = &self.sidebar_list;
        let unchanged = cache
            .workspaces
            .iter()
            .map(|(id, local, name, machine)| (*id, local.as_str(), name.as_str(), machine.as_deref()))
            .eq(self.board.workspaces.iter().map(|workspace| {
                (
                    workspace.id,
                    workspace.local_id.as_str(),
                    workspace.name.as_str(),
                    workspace.place.machine(),
                )
            }));
        if unchanged
            && cache.attention_version == attention_version
            && cache.read_epoch == cache.inbox_epoch
            && cache.refreshed.is_some_and(|at| now.duration_since(at) < REFRESH)
        {
            return;
        }
        let workspaces = self
            .board
            .workspaces
            .iter()
            .map(|workspace| {
                (
                    workspace.id,
                    workspace.local_id.clone(),
                    workspace.name.clone(),
                    workspace.place.machine().map(str::to_owned),
                )
            })
            .collect();
        let questions = self.read_questions();
        let mut turns = std::mem::take(&mut self.sidebar_list.turns);
        self.observe_orchestrator_turns(&mut turns);
        let rows = self.rows_from(&questions, &turns);
        self.sidebar_list = ListCache {
            refreshed: Some(now),
            attention_version,
            read_epoch: self.sidebar_list.inbox_epoch,
            inbox_epoch: self.sidebar_list.inbox_epoch,
            workspaces,
            rows,
            questions,
            turns,
        };
    }

    /// Keeps the turn of the orchestrator of each workspace, and clears the end
    /// of a turn that the dock shows now.
    fn observe_orchestrator_turns(&mut self, turns: &mut HashMap<String, TurnWatch>) {
        for workspace in &self.board.workspaces {
            let Some(turn) = self.orchestrator_dock.turn_of(&workspace.local_id) else {
                continue;
            };
            let shown = self.dock_shows(&workspace.local_id);
            turns
                .entry(workspace.local_id.clone())
                .or_default()
                .observe(turn, shown);
        }
    }

    /// Whether the dock stands open on the conversation of `local_id` in this
    /// frame, so its turns are seen.
    ///
    /// The dock shows the front desk or the active workspace, so the active
    /// workspace alone does not say that its conversation is on the screen.
    pub(in crate::app) fn dock_shows(&self, local_id: &str) -> bool {
        self.orchestrator_dock.visible() && self.orchestrator_dock.front_desk().key(&self.board) == local_id
    }

    /// The rows of the last read of the session, which a test reads: the
    /// heaviest attention state of each workspace, how many of its panels wait for
    /// the person, its machine and the state of its orchestrator.
    #[cfg(test)]
    pub(in crate::app) fn read_sidebar_rows(&self) -> HashMap<WorkspaceId, Row> {
        self.rows_from(&self.sidebar_list.questions, &self.sidebar_list.turns)
    }

    /// The rows of one read, from the questions of that read and the turns of the
    /// orchestrators of the workspaces.
    fn rows_from(&self, questions: &[Question], turns: &HashMap<String, TurnWatch>) -> HashMap<WorkspaceId, Row> {
        self.board
            .workspaces
            .iter()
            .map(|workspace| {
                let panels = workspace.panels.iter().filter_map(|id| self.board.panel(*id));
                let attention = Attention::heaviest(
                    panels
                        .clone()
                        .filter(|panel| panel.kind.is_agent())
                        .map(|panel| self.panel_attention_of(panel)),
                );
                let waiting = sufficient_waiting(
                    panels
                        .clone()
                        .filter(|panel| panel.kind.is_agent())
                        .map(|panel| self.panel_attention_of(panel)),
                );
                let asked = questions
                    .iter()
                    .filter(|question| question.workspace == workspace.id)
                    .count();
                let watch = turns.get(&workspace.local_id);
                let orchestrator = Orchestrator::of(
                    asked,
                    watch.map_or_else(Turn::default, TurnWatch::turn),
                    watch.and_then(TurnWatch::unseen),
                );
                (
                    workspace.id,
                    Row::placed(attention, waiting, workspace.place.machine(), orchestrator),
                )
            })
            .collect()
    }

    /// The group `Work items`: its caption with the two icon controls that replace
    /// the buttons `New` and `Fit`, and the row of each workspace of the board, in
    /// board order.
    pub(super) fn render_sidebar_workspaces(
        &mut self,
        ui: &mut egui::Ui,
        workspace_data: &[WorkspaceSidebarEntry],
        actions: &mut SidebarActions,
        drag_state: &mut SidebarWorkspaceDragState,
    ) {
        let fits = self.has_attached_workspace();
        let count = self
            .sidebar_chrome
            .counts
            .text(caption::WORK_ITEMS, workspace_data.len());
        let controls = [
            if fits {
                caption::Control::new(caption::Glyph::Fit, caption::FIT_HOVER)
            } else {
                caption::Control::disabled(caption::Glyph::Fit, caption::FIT_HOVER)
            },
            caption::Control::new(caption::Glyph::Plus, caption::NEW_HOVER),
        ];
        if let Some(press) = caption::group(ui, caption::WORK_ITEMS, Some(count), &controls) {
            actions.take_caption_press(super::Group::WorkItems, press);
        }
        for workspace in workspace_data {
            self.render_sidebar_workspace(ui, workspace, actions, drag_state);
        }
    }

    /// The row of `workspace` from the last read; a workspace read for the first
    /// time shows as idle until then.
    pub(super) fn sidebar_row(&self, workspace: WorkspaceId) -> Row {
        self.sidebar_list
            .rows
            .get(&workspace)
            .cloned()
            .unwrap_or_else(|| Row::local(Attention::Idle))
    }
}

/// How many agent panels of one workspace wait for the person.
fn sufficient_waiting(states: impl Iterator<Item = Attention>) -> Waiting {
    let mut waiting = Waiting::default();
    for state in states {
        match state {
            Attention::NeedsYou => waiting.needs_you += 1,
            Attention::Done => waiting.done += 1,
            Attention::Idle | Attention::Working | Attention::Background => {}
        }
    }
    waiting
}

/// Whether a workspace drops on the row `target`: on any docked row. A detached
/// workspace has its own window, so it takes no drop.
pub(super) fn accepts_drop(target: &WorkspaceSidebarEntry) -> bool {
    super::sidebar_workspace_drop_should_dock(target.detached)
}

#[cfg(test)]
mod tests {
    use horizon_core::Attention;
    use horizon_core::WorkspacePlace;
    use horizon_core::workspace_list::{Orchestrator, Turn, TurnEnd};

    use super::TurnWatch;

    #[test]
    fn a_turn_of_the_orchestrator_says_working_done_and_failed() {
        let mut watch = TurnWatch::default();
        let busy = Turn { busy: true, failed: 0 };
        let quiet = Turn { busy: false, failed: 0 };

        // A turn that runs, and then ends with the dock elsewhere.
        watch.observe(busy, false);
        assert_eq!(watch.turn(), busy);
        watch.observe(quiet, false);
        assert_eq!(watch.unseen(), Some(TurnEnd::Done), "a turn that ended well is unseen");

        // The person reads the conversation in the dock, so the end is seen.
        watch.observe(quiet, true);
        assert_eq!(watch.unseen(), None, "the dock showed the conversation");

        // A turn that fails holds the state of a question until the dock shows it.
        watch.observe(busy, false);
        assert_eq!(watch.unseen(), None, "a turn that runs is no end");
        watch.observe(Turn { busy: false, failed: 1 }, false);
        assert_eq!(
            watch.unseen(),
            Some(TurnEnd::Failed),
            "the line of a failed turn names it"
        );

        // The end of a turn of a conversation with two failures is known from the
        // count that the read before it saw.
        let mut watch = TurnWatch::default();
        watch.observe(Turn { busy: true, failed: 1 }, false);
        watch.observe(Turn { busy: false, failed: 1 }, false);
        assert_eq!(
            watch.unseen(),
            Some(TurnEnd::Done),
            "a turn that adds no failed line of its own ended well"
        );
    }

    #[test]
    fn the_word_of_the_orchestrator_joins_the_facts_that_the_row_holds_today() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("a");
        app.board
            .set_workspace_place(workspace, WorkspacePlace::Machine("build-1".to_owned()));
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    name: Some("Fix the sync".to_owned()),
                    kind: horizon_core::PanelKind::Claude,
                    command: Some("seed".to_owned()),
                    ..horizon_core::PanelOptions::default()
                },
                workspace,
            )
            .expect("panel");
        let local = app.board.panel(panel).expect("panel").local_id.clone();

        // An agent of the workspace asks a question of its own, and a card of the
        // workspace asks another one.
        app.board.focused = None;
        let mut ledger = horizon_core::agent_work::TurnLedger::default();
        ledger.session_id = "session".into();
        ledger.prompt_id = Some("prompt".into());
        ledger.state = horizon_core::agent_work::TurnState::Blocked;
        ledger.updated_at_millis = horizon_core::current_unix_millis() - 1;
        app.apply_attention_values(vec![(panel, local, Some(ledger))], &egui::Context::default());
        let workspace_local = app.board.workspace(workspace).expect("workspace").local_id.clone();
        app.orchestrator_dock.mark_running_for_test(&workspace_local);
        crate::app::sidebar::inbox::asking_card(&mut app, workspace, "Run the plan?", &[("go", "Go")]);

        app.refresh_sidebar_rows(std::time::Instant::now());
        let row = app.sidebar_row(workspace);
        assert_eq!(
            row.line, "Needs you \u{00B7} 1 needs you \u{00B7} on build-1",
            "the word comes first, then the count of the agents and the machine"
        );
        assert_eq!(row.attention, Attention::NeedsYou);

        // The state of the orchestrator alone takes the row, with no facts of the
        // agents behind it.
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("b");
        app.sidebar_list.turns.insert(
            app.board.workspace(workspace).expect("workspace").local_id.clone(),
            TurnWatch {
                busy: false,
                failed: 0,
                unseen: Some(TurnEnd::Done),
            },
        );
        app.refresh_sidebar_rows(std::time::Instant::now());
        assert_eq!(app.sidebar_row(workspace).line, "Done");
        assert_eq!(app.sidebar_row(workspace).attention, Attention::Done);
        assert_eq!(
            Orchestrator::of(0, Turn::default(), Some(TurnEnd::Done)).word(),
            Some("Done")
        );
    }

    #[test]
    fn the_dock_that_shows_a_conversation_clears_the_end_of_its_turn() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("a");
        app.board.focus_workspace(workspace);
        let local_id = app.board.workspace(workspace).expect("workspace").local_id.clone();

        // The test turns the dock on itself, so it starts from a window without it.
        app.orchestrator_dock = crate::app::orchestrator_dock::OrchestratorDock::new(false);
        app.orchestrator_dock.toggle();
        // A dock shows the front desk at its start; the person presses the row of
        // the workspace, and the dock then shows that conversation.
        assert!(
            !app.dock_shows(&local_id),
            "the front desk is no conversation of a workspace"
        );
        app.orchestrator_dock.front_desk_mut().show_workspace();
        assert!(app.dock_shows(&local_id), "the dock draws the active workspace");
        assert!(
            !app.dock_shows("another-workspace"),
            "another conversation is not shown"
        );

        // A workspace that is not active holds a conversation that the dock does
        // not show, so the end of its turn stays unseen.
        let other = app.board.create_workspace("b");
        app.board.focus_workspace(other);
        assert!(!app.dock_shows(&local_id));
        assert!(app.dock_shows(&app.board.workspace(other).expect("workspace").local_id.clone()));
    }

    #[test]
    fn a_replaced_session_with_the_same_workspace_ids_reads_the_rows_again() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let _workspace = app.board.create_workspace("a");
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);
        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(
            app.sidebar_list.refreshed,
            Some(start),
            "same workspaces: the cache stays"
        );

        // The workspaces of a replaced session get the same ids from 1.
        for workspace in &mut app.board.workspaces {
            workspace.local_id = format!("{}-other-session", workspace.local_id);
        }
        app.refresh_sidebar_rows(soon);
        assert_eq!(app.sidebar_list.refreshed, Some(soon));
    }

    #[test]
    fn the_end_of_a_turn_changes_the_word_of_the_row_on_the_next_frame_with_no_timer() {
        // A dock that shows the conversation clears the word `Done`, so this window
        // keeps no dock.
        let (_temp, mut app) = crate::app::test_support::test_app_without_dock();
        let workspace = app.board.create_workspace("a");
        let local_id = app.board.workspace(workspace).expect("workspace").local_id.clone();
        // The program of the dock is not there, so this test holds its turn: the
        // pass of the dock reads the turn of a conversation and asks the sidebar for
        // the rows again, and no frame of the sidebar asks for a clock.
        app.board.focus_workspace(workspace);
        let start = std::time::Instant::now();
        app.orchestrator_dock.begin_turn_for_test(&local_id);
        app.poll_orchestrator_dock(&egui::Context::default());
        app.refresh_sidebar_rows(start);
        assert_eq!(app.sidebar_row(workspace).line, "Working");

        // The turn ends, and the pass of the dock tells the sidebar on the frame
        // that reads it.
        app.orchestrator_dock.end_turn_for_test(&local_id);
        app.poll_orchestrator_dock(&egui::Context::default());
        assert_ne!(
            app.sidebar_list.read_epoch, app.sidebar_list.inbox_epoch,
            "the end of the turn is news for the rows"
        );
        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(
            app.sidebar_list.refreshed,
            Some(soon),
            "the next frame reads the rows, inside the second of the clock"
        );
        assert_eq!(app.sidebar_row(workspace).line, "Done", "the turn ended well");
    }

    #[test]
    fn a_changed_attention_state_reads_the_rows_again_at_once() {
        use horizon_core::Attention;

        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("a");
        let panel = app
            .board
            .create_panel(
                horizon_core::PanelOptions {
                    name: Some("Fix the sync".to_owned()),
                    kind: horizon_core::PanelKind::Claude,
                    command: Some("seed".to_owned()),
                    ..horizon_core::PanelOptions::default()
                },
                workspace,
            )
            .expect("panel");
        let local = app.board.panel(panel).expect("panel").local_id.clone();
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);
        assert_eq!(app.sidebar_row(workspace).attention, Attention::Idle);

        // A question opens in the ledger while the person looks elsewhere.
        app.board.focused = None;
        let mut ledger = horizon_core::agent_work::TurnLedger::default();
        ledger.session_id = "session".into();
        ledger.prompt_id = Some("prompt".into());
        ledger.state = horizon_core::agent_work::TurnState::Blocked;
        ledger.updated_at_millis = horizon_core::current_unix_millis() - 1;
        app.apply_attention_values(vec![(panel, local, Some(ledger))], &egui::Context::default());

        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(
            app.sidebar_list.refreshed,
            Some(soon),
            "the row of a workspace whose badge changed is read again at once"
        );
        assert_eq!(app.sidebar_row(workspace).attention, Attention::NeedsYou);
        assert_eq!(app.sidebar_row(workspace).waiting.needs_you, 1);
    }

    #[test]
    fn the_place_of_a_workspace_is_kept_in_its_cached_row() {
        let (_temp, mut app) = crate::app::test_support::test_app();
        let workspace = app.board.create_workspace("Build");
        let here = app.board.create_workspace("Here");
        let start = std::time::Instant::now();
        app.refresh_sidebar_rows(start);

        assert_eq!(
            app.sidebar_row(workspace).line,
            "",
            "a row of this computer has no line yet"
        );
        assert_eq!(app.sidebar_row(here).line, "");

        // A workspace that moves to a machine reads its rows again at once,
        // because the machine is part of what the cache holds.
        app.board
            .set_workspace_place(workspace, WorkspacePlace::Machine("build-1".to_owned()));
        let soon = start + std::time::Duration::from_millis(100);
        app.refresh_sidebar_rows(soon);
        assert_eq!(app.sidebar_list.refreshed, Some(soon));
        assert_eq!(app.sidebar_row(workspace).line, "on build-1");
        assert_eq!(
            app.sidebar_row(here).line,
            "",
            "this computer says nothing about a place"
        );

        // The composed line is a row text like the others: no frame builds it
        // again while the row is a second old.
        app.refresh_sidebar_rows(soon + std::time::Duration::from_millis(100));
        assert_eq!(app.sidebar_list.refreshed, Some(soon), "the cache stays");
    }
}
