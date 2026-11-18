use std::path::Path;
use std::time::{Duration, Instant};

use crate::agent_work::ProgramStatus;
use crate::board::Board;
use crate::panel::{PanelId, PanelKind};
use crate::runtime_state::RuntimeState;
use crate::terminal::{FollowFeed, FollowSpec};
use crate::{CanvasViewState, HorizonHome, WindowConfig};

fn spec(kind: PanelKind) -> FollowSpec {
    FollowSpec {
        device: "device-1".to_owned(),
        label: "other computer".to_owned(),
        panel: "panel-1".to_owned(),
        title: "review".to_owned(),
        kind,
        cols: 100,
        rows: 30,
        typing: true,
    }
}

/// The size of one cell of the terminal font, in points, as the terminal widget
/// measures it.
const CELL: [f32; 2] = [7.83, 16.9];

/// A board with one workspace and one follow panel in it.
fn board_with_a_follow_panel(kind: PanelKind) -> (Board, PanelId, FollowFeed) {
    let mut board = Board::new();
    let workspace = board.create_workspace("shared");
    let id = board
        .create_follow_panel(workspace, spec(kind), CELL)
        .expect("a follow panel opens");
    let feed = board.panel(id).and_then(crate::Panel::follow_feed).expect("its feed");
    (board, id, feed)
}

/// Takes the output of the board until `ready` holds.
fn wait_until(board: &mut Board, mut ready: impl FnMut(&Board) -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        board.process_output();
        if ready(board) {
            return;
        }
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_follow_panel_opens_at_the_grid_of_its_stream() {
    let (board, id, _feed) = board_with_a_follow_panel(PanelKind::Claude);
    let panel = board.panel(id).expect("the follow panel");

    assert!(panel.is_follower());
    assert_eq!(panel.follow_spec(), Some(&spec(PanelKind::Claude)));
    assert_eq!(panel.title, "review");
    let terminal = panel.terminal().expect("its terminal");
    assert_eq!((terminal.cols(), terminal.rows()), (100, 30));
    // The body of the panel holds 100 by 30 cells of [`CELL`] points, beside the
    // scrollbar and under the title bar.
    assert_eq!(
        panel.layout.size,
        [
            (100.0 + 1.0) * CELL[0] - 1.0 + 20.0 + 16.0,
            (30.0 + 1.0) * CELL[1] - 1.0 + 34.0 + 16.0
        ]
    );
    assert_eq!(board.focused, Some(id));
}

#[test]
fn a_board_saves_and_restores_without_its_follow_panel() {
    let (board, _id, _feed) = board_with_a_follow_panel(PanelKind::Shell);

    let state = RuntimeState::from_board(&board, WindowConfig::default(), CanvasViewState::default());

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.panel_count(), 0, "the saved session holds no follow panel");
    assert_eq!(state.focused_panel_local_id, None);
    let restored = Board::from_runtime_state(&state).expect("the saved board restores");
    assert!(restored.panels.is_empty());
}

#[test]
fn the_status_of_the_stream_shows_on_the_follow_panel() {
    let (mut board, id, feed) = board_with_a_follow_panel(PanelKind::Codex);

    feed.output(b"\x1b]7501;state=working:app=codex\x1b\\\x1b]0;remote title\x07");

    wait_until(
        &mut board,
        |board| {
            board
                .panel(id)
                .is_some_and(|panel| panel.terminal_title == "remote title")
        },
        "the title of the stream arrives",
    );
    let panel = board.panel(id).expect("the follow panel");
    assert_eq!(panel.program_status(), ProgramStatus::Working);
    assert_eq!(panel.display_title(), "remote title");
}

#[test]
fn a_follow_panel_says_that_its_stream_ended() {
    let (board, id, feed) = board_with_a_follow_panel(PanelKind::Shell);
    let panel = board.panel(id).expect("the follow panel");
    assert_eq!(panel.follow_ended_line(), None);

    feed.ended("the link to the server is lost");
    assert_eq!(
        panel.follow_ended_line().as_deref(),
        Some("The stream of this terminal ended: the link to the server is lost. Text typed here is not sent.")
    );

    feed.ended("");
    assert_eq!(
        panel.follow_ended_line().as_deref(),
        Some("The stream of this terminal ended. Text typed here is not sent.")
    );
}

/// The places that act on the program of a panel do nothing for a follow panel,
/// also when it shows the panel of an agent.
#[test]
fn nothing_for_a_started_program_runs_for_a_follow_panel() {
    let (mut board, id, feed) = board_with_a_follow_panel(PanelKind::Claude);
    let workspace = board.panel_workspace_id(id).expect("its workspace");

    // No agent rule matches the panel, and it holds nothing of a launch.
    let panel = board.panel(id).expect("the follow panel");
    assert_eq!(panel.kind, PanelKind::Shell);
    assert!(!panel.kind.is_agent() && !panel.kind.supports_session_binding());
    assert!(panel.session_binding.is_none() && panel.session_id().is_none());
    assert!(panel.launch_command.is_none() && panel.launch_args.is_empty());
    assert!(panel.opened_by_actor().is_none() && panel.machine_session().is_none());
    // No process, no holder, no work of an agent.
    let terminal = panel.terminal().expect("its terminal");
    assert_eq!(terminal.child_pid(), None);
    assert!(
        terminal.hanging_child().is_none(),
        "no signal reaches a process at the close"
    );
    assert!(terminal.owned_process().is_none());
    assert!(terminal.work_owner.is_none() && terminal.pending_work_resume().is_none());

    // No working folder: not of the workspace, and not of a process.
    assert!(board.set_workspace_directory(workspace, Some(Path::new("/"))));
    feed.output(b"output of the stream");
    wait_until(
        &mut board,
        |board| board.panel(id).is_some_and(crate::Panel::drew_output),
        "the output of the stream arrives",
    );
    assert!(board.cwd_reads().is_empty());
    assert!(board.panel(id).is_some_and(|panel| panel.launch_cwd.is_none()));

    // No restart, so no resume and no program of this computer in the panel.
    let panel = board.panel_mut(id).expect("the follow panel");
    let home = HorizonHome::from_root("/nonexistent/horizon-home-of-a-follow-panel".into());
    assert!(panel.restart_request(home, true).expect("no refusal").is_none());
    board.restart_panel(id).expect("a restart that does nothing");
    board.close_held_programs();
    let panel = board.panel(id).expect("the follow panel");
    assert!(panel.is_follower() && panel.program_end().is_none() && !feed.closed());

    // The exit of Horizon waits for no program of a follow panel.
    assert_eq!(board.begin_async_shutdown().panel_count(), 0);

    board.close_panel(id);
    assert!(feed.closed(), "the feed knows that the person closed the panel");
}
