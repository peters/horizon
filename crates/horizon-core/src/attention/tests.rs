use super::{Attention, AttentionBoard, AttentionTarget, next_attention_target};
use crate::PanelId;
use crate::agent_work::{TurnLedger, TurnState};

fn ledger(state: TurnState, updated_at_millis: i64) -> TurnLedger {
    let mut ledger = TurnLedger::default();
    ledger.session_id = "session".into();
    ledger.prompt_id = Some("prompt".into());
    ledger.state = state;
    ledger.updated_at_millis = updated_at_millis;
    ledger
}

fn board() -> AttentionBoard {
    AttentionBoard::default()
}

#[test]
fn every_turn_state_of_a_ledger_has_its_attention_state() {
    let cases = [
        (TurnState::Working, Attention::Working),
        (TurnState::Blocked, Attention::NeedsYou),
        (TurnState::Failed, Attention::NeedsYou),
        (TurnState::Finished, Attention::Done),
        (TurnState::Interrupted, Attention::Done),
        (TurnState::Unknown, Attention::Idle),
    ];
    for (state, expected) in cases {
        let mut board = board();
        // The turn ran first, so its end is a change that the person missed.
        board.observe_ledger("panel", &ledger(TurnState::Working, 10), 10);
        board.observe_ledger("panel", &ledger(state, 20), 20);
        assert_eq!(board.state("panel"), expected, "{state:?}");
        assert!(board.from_ledger("panel"));
    }
}

#[test]
fn a_finished_turn_is_done_until_the_person_looks() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 10), 10);
    assert_eq!(board.state("panel"), Attention::Working);
    assert!(board.observe_ledger("panel", &ledger(TurnState::Finished, 20), 20));
    assert_eq!(board.state("panel"), Attention::Done);
    assert!(board.note_look("panel", 30));
    assert_eq!(board.state("panel"), Attention::Idle);
}

#[test]
fn an_open_question_survives_a_look_and_a_look_after_the_answer_clears_it() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Blocked, 20), 10);
    assert_eq!(board.state("panel"), Attention::NeedsYou);
    assert!(!board.note_look("panel", 30), "a question is not read away");
    assert_eq!(board.state("panel"), Attention::NeedsYou);
    // The ledger says that the answer arrived: the turn runs again.
    assert!(board.observe_ledger("panel", &ledger(TurnState::Working, 40), 40));
    assert_eq!(board.state("panel"), Attention::Working);
    board.observe_ledger("panel", &ledger(TurnState::Finished, 50), 50);
    assert_eq!(board.state("panel"), Attention::Done);
}

#[test]
fn a_failed_turn_waits_for_the_person_and_a_look_clears_it() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 10), 10);
    board.observe_ledger("panel", &ledger(TurnState::Failed, 20), 20);
    assert_eq!(board.state("panel"), Attention::NeedsYou);
    assert!(board.note_look("panel", 30));
    assert_eq!(board.state("panel"), Attention::Idle);
}

#[test]
fn background_work_keeps_the_panel_working_after_its_turn_ended() {
    let mut board = board();
    let mut finished = ledger(TurnState::Finished, 20);
    finished.background_tasks = 2;
    board.observe_ledger("panel", &finished, 10);
    assert_eq!(board.state("panel"), Attention::Background);
    assert_eq!(board.state("panel").word(), Some("Background"));
    assert_eq!(
        board.state("panel").spoken(),
        Some("Works in the background"),
        "a screen reader hears the whole state"
    );
    assert_eq!(
        board.state("panel").rank(),
        Attention::Working.rank(),
        "a workspace row shows background work as work"
    );
    board.note_look("panel", 30);
    assert_eq!(
        board.state("panel"),
        Attention::Background,
        "a look does not end background work"
    );
}

#[test]
fn a_read_that_is_older_than_the_one_the_panel_holds_changes_nothing() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 40), 10);
    let older = ledger(TurnState::Finished, 20);
    assert!(
        !board.observe_ledger("panel", &older, 50),
        "an older read cannot rewrite the state"
    );
    assert_eq!(board.state("panel"), Attention::Working);
    assert!(
        !board.observe_ledger("panel", &older, 60),
        "the same older read stays behind the state the panel holds"
    );
    assert_eq!(board.state("panel"), Attention::Working);
}

#[test]
fn a_relaunch_of_the_panel_shows_from_its_own_start() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 40), 10);
    board.observe_ledger("panel", &ledger(TurnState::Finished, 50), 50);
    assert_eq!(board.state("panel"), Attention::Done);
    // A new session of the same panel: its record starts before the turn that
    // the person saw end, and that turn is not shown as unread again.
    let mut relaunched = ledger(TurnState::Unknown, 0);
    relaunched.session_id = "a fresh session".into();
    relaunched.generation = 1;
    assert!(board.observe_ledger("panel", &relaunched, 60));
    assert_eq!(board.state("panel"), Attention::Idle);
}

#[test]
fn a_clock_that_went_back_leaves_the_panel_idle() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 40), 10);
    // A ledger that saw its clock go back holds unknown at the moment it
    // already recorded, because it cannot say which turn is current.
    board.observe_ledger("panel", &ledger(TurnState::Unknown, 40), 5);
    assert_eq!(board.state("panel"), Attention::Idle);
}

#[test]
fn the_first_read_of_a_panel_is_not_an_unread_turn() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Finished, 20), 30);
    assert_eq!(
        board.state("panel"),
        Attention::Idle,
        "a panel that the board reads for the first time shows no unread turn"
    );
}

#[test]
fn a_panel_without_hooks_reads_its_working_indicator() {
    let mut board = board();
    assert!(board.observe_working("panel", true, true, 10));
    assert_eq!(board.state("panel"), Attention::Working);
    assert!(!board.observe_working("panel", true, true, 11), "the same indicator");
    assert!(!board.from_ledger("panel"));
    // The turn ends while the person looks at another panel.
    assert!(board.observe_working("panel", false, false, 20));
    assert_eq!(board.state("panel"), Attention::Done);
    assert!(board.note_look("panel", 30));
    assert_eq!(board.state("panel"), Attention::Idle);
    // A turn that ends while the panel has the focus is read at once.
    board.observe_working("panel", true, true, 40);
    board.observe_working("panel", false, true, 50);
    assert_eq!(board.state("panel"), Attention::Idle);
}

#[test]
fn a_ledger_of_a_panel_wins_over_its_working_indicator() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 10), 10);
    board.observe_ledger("panel", &ledger(TurnState::Finished, 20), 20);
    assert_eq!(board.state("panel"), Attention::Done);
    board.observe_working("panel", true, false, 30);
    assert_eq!(
        board.state("panel"),
        Attention::Done,
        "the hooks know the turn, so the screen cannot reopen it"
    );
}

#[test]
fn marking_unread_marks_an_idle_agent_panel_and_a_look_clears_it() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 10), 10);
    board.observe_ledger("panel", &ledger(TurnState::Finished, 20), 20);
    board.note_look("panel", 30);
    assert_eq!(board.state("panel"), Attention::Idle);
    assert!(board.mark_unread("panel"));
    assert_eq!(board.state("panel"), Attention::Done);
    assert!(!board.mark_unread("panel"), "the panel already shows it");
    assert!(board.note_look("panel", 40));
    assert_eq!(board.state("panel"), Attention::Idle);
}

#[test]
fn a_panel_that_left_the_board_is_idle_and_is_read_again_from_nothing() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Blocked, 20), 10);
    board.forget("panel");
    assert_eq!(board.state("panel"), Attention::Idle);
    assert!(!board.from_ledger("panel"));
}

#[test]
fn a_workspace_shows_the_heaviest_state_of_its_panels() {
    assert_eq!(Attention::heaviest([]), Attention::Idle);
    assert_eq!(
        Attention::heaviest([Attention::Idle, Attention::Working, Attention::Background]),
        Attention::Working
    );
    assert_eq!(
        Attention::heaviest([Attention::Working, Attention::Done, Attention::Idle]),
        Attention::Done
    );
    assert_eq!(
        Attention::heaviest([Attention::Done, Attention::NeedsYou, Attention::Background]),
        Attention::NeedsYou
    );
}

fn target(panel: u64, attention: Attention, since_millis: i64) -> AttentionTarget {
    AttentionTarget {
        panel: PanelId(panel),
        attention,
        since_millis,
    }
}

#[test]
fn the_jump_takes_the_oldest_panel_that_needs_the_person_then_the_oldest_done() {
    let targets = [
        target(1, Attention::Done, 5),
        target(2, Attention::NeedsYou, 40),
        target(3, Attention::Working, 1),
        target(4, Attention::NeedsYou, 20),
        target(5, Attention::Done, 2),
    ];
    assert_eq!(next_attention_target(&targets, None), Some(PanelId(4)));
    // The next presses go on to the panel after the one that the person just
    // went to, so they reach the panels that finished unseen and come round.
    assert_eq!(next_attention_target(&targets, Some(PanelId(4))), Some(PanelId(2)));
    assert_eq!(next_attention_target(&targets, Some(PanelId(2))), Some(PanelId(5)));
    assert_eq!(next_attention_target(&targets, Some(PanelId(5))), Some(PanelId(1)));
    assert_eq!(
        next_attention_target(&targets, Some(PanelId(1))),
        Some(PanelId(4)),
        "after the last panel that waits, the panel that waited longest comes round again"
    );
    let done_only = [
        target(1, Attention::Done, 5),
        target(5, Attention::Done, 2),
        target(3, Attention::Working, 1),
    ];
    assert_eq!(next_attention_target(&done_only, Some(PanelId(5))), Some(PanelId(1)));
}

#[test]
fn the_jump_reaches_the_focused_panel_when_it_is_the_only_one() {
    let targets = [target(2, Attention::NeedsYou, 40)];
    assert_eq!(next_attention_target(&targets, Some(PanelId(2))), Some(PanelId(2)));
    assert_eq!(next_attention_target(&[], None), None);
    assert_eq!(
        next_attention_target(&[target(3, Attention::Working, 1)], None),
        None,
        "working panels and background work are not the person's to answer"
    );
    assert_eq!(
        next_attention_target(&[target(3, Attention::Background, 1)], None),
        None
    );
}

/// The record of a launch whose hooks have not run yet: the store registers the
/// session and the generation before the agent starts, and no event has set the
/// moment of the last one.
fn quiet_ledger(session: &str, generation: u64) -> TurnLedger {
    let mut ledger = TurnLedger::default();
    ledger.session_id = session.into();
    ledger.generation = generation;
    ledger
}

#[test]
fn a_launch_that_heard_no_event_keeps_the_screen_rule() {
    let mut board = board();
    board.observe_ledger("panel", &quiet_ledger("session", 1), 10);
    assert!(!board.from_ledger("panel"), "no event has arrived");
    assert_eq!(board.state("panel"), Attention::Idle);
    // The screen of the panel decides, exactly as for a panel without hooks.
    assert!(board.observe_working("panel", true, false, 20));
    assert_eq!(board.state("panel"), Attention::Working);
    // A late read of the same quiet launch changes nothing.
    assert!(!board.observe_ledger("panel", &quiet_ledger("session", 1), 30));
    assert_eq!(board.state("panel"), Attention::Working);
    assert!(board.observe_working("panel", false, false, 40));
    assert_eq!(board.state("panel"), Attention::Done);
    assert!(!board.from_ledger("panel"));
}

#[test]
fn the_first_event_of_a_launch_switches_the_panel_to_its_ledger() {
    let mut board = board();
    board.observe_ledger("panel", &quiet_ledger("session", 1), 10);
    board.observe_working("panel", true, false, 20);
    assert_eq!(board.state("panel"), Attention::Working);
    // The agent starts: its session start is the first event of the launch.
    let mut session_start = ledger(TurnState::Unknown, 30);
    session_start.generation = 1;
    assert!(board.observe_ledger("panel", &session_start, 30));
    assert_eq!(board.state("panel"), Attention::Idle, "the ledger answers now");
    assert!(board.from_ledger("panel"));
    // The screen cannot reopen what the ledger holds.
    board.observe_working("panel", true, false, 35);
    assert_eq!(board.state("panel"), Attention::Idle);
    // And the turn of the ledger is read as a panel with hooks reads it.
    let mut working = ledger(TurnState::Working, 40);
    working.generation = 1;
    board.observe_ledger("panel", &working, 40);
    assert_eq!(board.state("panel"), Attention::Working);
}

#[test]
fn a_stale_event_of_an_older_launch_does_not_switch_the_panel() {
    let mut board = board();
    // The panel runs its second launch and has heard no event of it.
    board.observe_ledger("panel", &quiet_ledger("second", 2), 10);
    board.observe_working("panel", true, false, 20);
    assert_eq!(board.state("panel"), Attention::Working);
    // A read of the launch before it arrives late, with an event of its own. It
    // describes a panel process that is gone.
    let mut older = ledger(TurnState::Blocked, 40);
    older.generation = 1;
    assert!(!board.observe_ledger("panel", &older, 50));
    assert!(!board.from_ledger("panel"), "the older launch cannot answer");
    assert_eq!(board.state("panel"), Attention::Working);
}

#[test]
fn a_relaunch_begins_again_with_the_screen_rule() {
    let mut board = board();
    let mut working = ledger(TurnState::Working, 10);
    working.generation = 1;
    board.observe_ledger("panel", &working, 10);
    assert!(board.from_ledger("panel"));
    // A new process of the same panel registered a new launch, and its hooks
    // have not run yet.
    board.observe_ledger("panel", &quiet_ledger("session", 2), 20);
    assert!(!board.from_ledger("panel"));
    assert_eq!(board.state("panel"), Attention::Idle);
    assert!(board.observe_working("panel", true, false, 30));
    assert_eq!(board.state("panel"), Attention::Working);
}

/// The rule of the owner beside the screen: an exact source decides, and the
/// working indicator is a guess. The stale timer of a spinner therefore cannot
/// end a turn that the record holds open, and a spinner cannot reopen a turn
/// that the record closed. A Claude launch and a pi launch share this path.
#[test]
fn the_screen_of_a_panel_with_a_report_cannot_end_or_reopen_its_turn() {
    let mut board = board();
    board.observe_ledger("panel", &ledger(TurnState::Working, 10), 10);
    assert_eq!(board.state("panel"), Attention::Working);
    // The agent worked for two seconds without a repaint of its spinner, which
    // is the stale timer of the working indicator.
    assert!(!board.observe_working("panel", false, false, 12));
    assert_eq!(
        board.state("panel"),
        Attention::Working,
        "a turn that the report holds open needs no spinner"
    );
    board.observe_ledger("panel", &ledger(TurnState::Finished, 20), 20);
    assert_eq!(board.state("panel"), Attention::Done);
    // A line of the screen that looks like a working indicator is the guess of
    // the fallback, and the record is exact.
    assert!(!board.observe_working("panel", true, false, 30));
    assert_eq!(
        board.state("panel"),
        Attention::Done,
        "a spinner cannot reopen a turn that the report closed"
    );
}
