use super::{MAX_REPORT_BYTES, ProgramStatus, program_status, program_status_reports_state};
use crate::PanelKind;
use crate::agent_work::ProgramClaim;
use crate::agent_work::TurnState;

/// The state that the payload of a codex panel claims.
fn codex(payload: &str) -> ProgramStatus {
    program_status(PanelKind::Codex, payload.as_bytes())
}

#[test]
fn every_state_of_the_protocol_is_read() {
    for (payload, expected) in [
        ("state=idle:app=codex", ProgramStatus::Idle),
        ("state=working:app=codex", ProgramStatus::Working),
        ("state=blocked:app=codex:kind=permission", ProgramStatus::Blocked),
        ("state=done:app=codex", ProgramStatus::Done),
        ("state=error:app=codex", ProgramStatus::Failed),
        ("state=clear:app=codex", ProgramStatus::Clear),
    ] {
        assert_eq!(codex(payload), expected, "the payload {payload}");
    }
}

#[test]
fn the_pairs_of_a_report_may_come_in_any_order() {
    for payload in [
        "state=working:app=codex",
        "app=codex:state=working",
        "app=codex:state=working:progress=40",
        " state = working : app = codex ",
    ] {
        assert_eq!(codex(payload), ProgramStatus::Working, "the payload {payload}");
    }
}

#[test]
fn the_wait_of_a_report_is_one_word_whatever_its_kind_is() {
    for kind in ["permission", "question", "auth", "unknown"] {
        assert_eq!(
            codex(&format!("state=blocked:app=codex:kind={kind}")),
            ProgramStatus::Blocked,
            "the kind {kind}"
        );
    }
    assert_eq!(codex("state=blocked:app=codex"), ProgramStatus::Blocked);
}

#[test]
fn a_report_of_another_program_or_without_one_claims_nothing() {
    for payload in [
        "state=working:app=cargo",
        "state=working",
        "state=working:app=",
        "state=working:app=codex2",
    ] {
        assert_eq!(codex(payload), ProgramStatus::Unknown, "the payload {payload}");
    }
}

#[test]
fn a_record_beneath_the_root_claims_nothing() {
    assert_eq!(codex("state=working:app=codex:id=build"), ProgramStatus::Unknown);
    assert_eq!(codex("state=working:app=codex:id="), ProgramStatus::Working);
}

#[test]
fn an_unknown_state_or_no_state_claims_nothing() {
    for payload in ["state=running:app=codex", "app=codex", "state=:app=codex"] {
        assert_eq!(codex(payload), ProgramStatus::Unknown, "the payload {payload}");
    }
}

#[test]
fn a_pair_that_the_protocol_forbids_is_skipped_and_the_rest_is_read() {
    for payload in [
        "state=working:app=codex:no separator",
        "state=working:app=codex:KIND=permission",
        "state=working:app=codex:kind=per mission",
        "state=working:app=codex:a_really_long_key_name_here=1",
    ] {
        assert_eq!(codex(payload), ProgramStatus::Working, "the payload {payload}");
    }
}

#[test]
fn the_last_value_of_a_repeated_key_wins() {
    assert_eq!(codex("state=done:app=codex:state=working"), ProgramStatus::Working);
    assert_eq!(codex("state=working:app=cargo:app=codex"), ProgramStatus::Working);
}

#[test]
fn a_panel_whose_program_reports_nothing_reads_no_report() {
    assert!(!program_status_reports_state(PanelKind::Shell));
    assert_eq!(
        program_status(PanelKind::Shell, b"state=working:app=codex"),
        ProgramStatus::Unknown
    );
    assert!(program_status_reports_state(PanelKind::Codex));
}

#[test]
fn bytes_that_are_not_utf8_or_beyond_the_bound_claim_nothing() {
    assert_eq!(
        program_status(PanelKind::Codex, b"state=working:app=codex:\xff"),
        ProgramStatus::Unknown
    );
    let payload = format!("state=working:app=codex:{}", "a".repeat(MAX_REPORT_BYTES));
    assert_eq!(codex(&payload), ProgramStatus::Unknown);
}

/// The claim of a launch that is ready, after the reports of `states`.
///
/// The program of a launch reports `idle` when it is ready for the person, and the
/// reports before that one are its start-up. Every test of a turn therefore starts
/// from that report.
fn claimed(states: &[ProgramStatus]) -> ProgramClaim {
    let mut claim = ProgramClaim::default();
    claim.hear(ProgramStatus::Idle, 900);
    for (index, state) in states.iter().enumerate() {
        claim.hear(*state, 1_000 + i64::try_from(index).unwrap_or(0));
    }
    claim
}

#[test]
fn the_words_of_a_turn_reach_the_four_states() {
    assert_eq!(claimed(&[ProgramStatus::Working]).ledger().state, TurnState::Working);
    assert_eq!(claimed(&[ProgramStatus::Blocked]).ledger().state, TurnState::Blocked);
    assert_eq!(
        claimed(&[ProgramStatus::Working, ProgramStatus::Blocked])
            .ledger()
            .state,
        TurnState::Blocked
    );
    assert_eq!(
        claimed(&[ProgramStatus::Working, ProgramStatus::Blocked, ProgramStatus::Working])
            .ledger()
            .state,
        TurnState::Working
    );
    assert_eq!(
        claimed(&[ProgramStatus::Working, ProgramStatus::Done]).ledger().state,
        TurnState::Finished
    );
    assert_eq!(
        claimed(&[ProgramStatus::Working, ProgramStatus::Failed]).ledger().state,
        TurnState::Failed
    );
    assert_eq!(
        claimed(&[ProgramStatus::Working, ProgramStatus::Idle]).ledger().state,
        TurnState::Interrupted
    );
}

#[test]
fn a_wait_that_an_idle_report_ends_is_an_interrupted_turn() {
    // The person denies the approval: the program reports idle and no done.
    let denied = claimed(&[ProgramStatus::Working, ProgramStatus::Blocked, ProgramStatus::Idle]);
    assert_eq!(denied.ledger().state, TurnState::Interrupted, "a denial stops the turn");
    assert!(denied.ledger().prompt_id.is_some(), "the turn keeps its identity");

    // An approval that the person allows and a turn that then finishes is a finish.
    let allowed = claimed(&[ProgramStatus::Working, ProgramStatus::Blocked, ProgramStatus::Done]);
    assert_eq!(allowed.ledger().state, TurnState::Finished);
}

#[test]
fn a_launch_that_reports_once_answers_for_its_panel() {
    assert!(
        claimed(&[ProgramStatus::Idle]).answers(),
        "an idle report is the protocol at work"
    );
    assert!(!ProgramClaim::default().answers());
    assert!(!claimed(&[ProgramStatus::Clear]).answers(), "a clear leaves no claim");
}

#[test]
fn a_report_of_no_turn_opens_and_closes_nothing() {
    let done = claimed(&[ProgramStatus::Done]);
    assert_eq!(done.ledger().state, TurnState::Unknown);
    assert!(done.answers(), "the launch reported, so its title is no longer read");

    let idle = claimed(&[ProgramStatus::Idle]);
    assert_eq!(idle.ledger().state, TurnState::Unknown);

    let failed = claimed(&[ProgramStatus::Failed]);
    assert_eq!(failed.ledger().state, TurnState::Unknown);
}

#[test]
fn a_repeated_report_of_the_state_that_stands_does_no_work() {
    let claim = claimed(&[ProgramStatus::Working]);
    let mut repeated = claim.clone();
    for index in 0..1_000 {
        repeated.hear(ProgramStatus::Working, 2_000 + index);
    }
    assert_eq!(repeated.ledger().updated_at_millis, claim.ledger().updated_at_millis);
}

#[test]
fn a_late_report_cannot_close_the_turn_of_a_later_one() {
    let mut claim = claimed(&[ProgramStatus::Working, ProgramStatus::Done]);
    let closed = claim.ledger().updated_at_millis;
    // A `done` of the turn that already ended, and one of a turn that never ran.
    claim.hear(ProgramStatus::Done, closed + 1);
    assert_eq!(claim.ledger().state, TurnState::Finished);
    assert_eq!(claim.ledger().updated_at_millis, closed, "a repeated end does no work");

    // A new turn opens, and its own `done` is the one that ends it.
    claim.hear(ProgramStatus::Working, closed + 2);
    assert_eq!(claim.ledger().state, TurnState::Working);
    assert_ne!(claim.ledger().prompt_id.as_deref(), Some("status-1"));
    claim.hear(ProgramStatus::Done, closed + 3);
    assert_eq!(claim.ledger().state, TurnState::Finished);
}

#[test]
fn a_clear_forgets_the_launch() {
    let mut claim = claimed(&[ProgramStatus::Working, ProgramStatus::Blocked, ProgramStatus::Clear]);
    assert!(!claim.answers());
    assert_eq!(claim.ledger().state, TurnState::Unknown);
    assert!(claim.ledger().prompt_id.is_none());

    // The next turn of the launch starts from its own count.
    claim.hear(ProgramStatus::Working, 5_000);
    assert_eq!(claim.ledger().prompt_id.as_deref(), Some("status-1"));
    assert_eq!(claim.ledger().state, TurnState::Working);
}

#[test]
fn a_repeated_clear_does_no_work() {
    let mut claim = claimed(&[ProgramStatus::Clear]);
    let before = claim.clone();
    claim.hear(ProgramStatus::Clear, 9_000);
    assert_eq!(claim.ledger().state, before.ledger().state);
    assert_eq!(claim.ledger().updated_at_millis, before.ledger().updated_at_millis);
    assert!(!claim.answers());
}

#[test]
fn the_start_up_of_a_launch_claims_no_turn() {
    let mut claim = ProgramClaim::default();
    for index in 0..8 {
        claim.hear(ProgramStatus::Working, 100 + index);
        assert!(!claim.ready(), "the program is loading");
        assert!(!claim.answers(), "the launch claims nothing before its idle report");
        assert_eq!(claim.ledger().state, TurnState::Unknown);
    }
    // The idle report is the moment the program is ready, and no turn of the
    // person ran before it.
    claim.hear(ProgramStatus::Idle, 900);
    assert!(claim.ready());
    assert_eq!(claim.ledger().state, TurnState::Unknown);
    assert!(claim.answers(), "the launch speaks the protocol");

    claim.hear(ProgramStatus::Working, 1_000);
    claim.hear(ProgramStatus::Done, 1_100);
    assert_eq!(
        claim.ledger().state,
        TurnState::Finished,
        "a turn after the ready report"
    );
}

#[test]
fn a_question_of_a_launch_before_it_is_ready_claims_the_person() {
    let mut claim = ProgramClaim::default();
    claim.hear(ProgramStatus::Working, 100);
    claim.hear(ProgramStatus::Blocked, 200);
    assert_eq!(claim.ledger().state, TurnState::Blocked, "the person must answer it");
    assert!(claim.ledger().prompt_id.is_some(), "the question belongs to a turn");

    // The program is ready after the person answered: no turn of theirs ran in the
    // question, so its end is no **Done**.
    claim.hear(ProgramStatus::Idle, 300);
    assert_eq!(claim.ledger().state, TurnState::Unknown);
    assert!(claim.answers());
}

#[test]
fn a_report_that_claims_nothing_changes_nothing() {
    let mut claim = claimed(&[ProgramStatus::Working]);
    let before = claim.ledger().clone();
    claim.hear(ProgramStatus::Unknown, 9_000);
    assert_eq!(claim.ledger().state, before.state);
    assert_eq!(claim.ledger().updated_at_millis, before.updated_at_millis);
}
