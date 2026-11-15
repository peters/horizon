//! The matcher of a state title, and the claim that its states make.
use super::{MAX_STATE_TITLE_BYTES, TITLE_SETTLE_MILLIS, TitleClaim, TitleState, title_reports_state, title_state};
use crate::PanelKind;
use crate::agent_work::TurnState;

/// A Codex title with the spinner frame of the first moment of the day.
const SPINNER: char = '\u{280b}';

fn spinner_title(folder: &str) -> String {
    format!("{SPINNER} {folder}")
}

#[test]
fn the_braille_frames_of_a_working_title_claim_work() {
    for title in [
        spinner_title("work"),
        spinner_title("a folder with spaces"),
        spinner_title("[a folder in brackets]"),
        spinner_title("a folder | with a bar"),
        spinner_title("\u{2801}a folder that starts with a frame"),
        SPINNER.to_string(),
    ] {
        assert_eq!(title_state(PanelKind::Codex, &title), TitleState::Working, "{title:?}");
    }
}

#[test]
fn another_spinner_set_or_a_plain_title_claims_nothing() {
    for title in [
        "\u{2733} work".to_string(),
        "\u{273b} work".to_string(),
        "- work".to_string(),
        "work".to_string(),
        "Action Required".to_string(),
        "[x] Action Required | Create a file | work".to_string(),
        "[ . ] Action Required is what I wrote".to_string(),
        " ".to_string(),
        String::new(),
    ] {
        assert_eq!(title_state(PanelKind::Codex, &title), TitleState::Plain, "{title:?}");
    }
}

#[test]
fn both_frames_of_an_approval_claim_the_person_and_their_text_is_not_read() {
    for title in [
        "[ . ] Action Required | Create probe file | work".to_string(),
        "[ ! ] Action Required | Create a file | [work]".to_string(),
        "[ . ] Action Required | a request | a folder | with bars".to_string(),
        "[ ! ] Action Required".to_string(),
        "[ . ] Action Required | ".to_string(),
    ] {
        assert_eq!(title_state(PanelKind::Codex, &title), TitleState::NeedsYou, "{title:?}");
    }
}

#[test]
fn a_title_beyond_the_bound_or_cut_in_a_glyph_claims_nothing() {
    let longest = format!("{SPINNER} {}", "x".repeat(MAX_STATE_TITLE_BYTES - 4));
    assert_eq!(longest.len(), MAX_STATE_TITLE_BYTES);
    assert_eq!(title_state(PanelKind::Codex, &longest), TitleState::Working);

    let longer = format!("{longest} y");
    assert_eq!(title_state(PanelKind::Codex, &longer), TitleState::Plain);

    let approval = format!("[ . ] Action Required | {} | work", "x".repeat(MAX_STATE_TITLE_BYTES));
    assert_eq!(title_state(PanelKind::Codex, &approval), TitleState::Plain);

    // The bytes of a braille glyph without its last byte arrive as the
    // replacement character, and the title claims nothing.
    let cut = String::from_utf8_lossy(&[0xe2, 0xa0]);
    assert_eq!(title_state(PanelKind::Codex, &cut), TitleState::Plain);
    let cut_after = format!("{cut} work");
    assert_eq!(title_state(PanelKind::Codex, &cut_after), TitleState::Plain);
}

#[test]
fn only_the_kind_of_an_agent_with_a_state_title_has_the_rule() {
    assert!(title_reports_state(PanelKind::Codex));
    for kind in [
        PanelKind::Shell,
        PanelKind::Claude,
        PanelKind::Pi,
        PanelKind::Grok,
        PanelKind::OpenCode,
        PanelKind::Gemini,
        PanelKind::KiloCode,
        PanelKind::Ssh,
        PanelKind::Editor,
    ] {
        assert!(!title_reports_state(kind), "{kind:?}");
        assert_eq!(title_state(kind, &spinner_title("work")), TitleState::Plain, "{kind:?}");
    }
}

/// The claim of a launch that is ready, which its first plain title made so.
fn ready_claim() -> TitleClaim {
    let mut claim = TitleClaim::default();
    claim.hear(TitleState::Plain, 0);
    assert!(claim.ready(), "the plain title of a launch is its ready title");
    claim
}

#[test]
fn a_thousand_spinner_frames_of_one_state_do_no_work() {
    let mut claim = ready_claim();
    assert!(!claim.answers(), "no title of this launch has claimed a state");
    claim.hear(TitleState::Working, 1_000);
    let first = claim.ledger().clone();
    assert!(claim.answers());
    for frame in 0..1_000_i64 {
        claim.hear(TitleState::Working, 1_001 + frame);
        assert!(!claim.settle(1_001 + frame), "no plain title stands");
    }
    assert_eq!(*claim.ledger(), first, "a repeated frame changes no field");
}

#[test]
fn a_wait_and_the_work_after_it_stay_in_the_turn_that_opened_them() {
    let mut claim = ready_claim();
    claim.hear(TitleState::Working, 10);
    let opened = claim.ledger().prompt_id.clone();
    assert_eq!(claim.ledger().state, TurnState::Working);

    claim.hear(TitleState::NeedsYou, 20);
    assert_eq!(claim.ledger().state, TurnState::Blocked);
    assert_eq!(claim.ledger().prompt_id, opened, "the wait belongs to the turn");

    claim.hear(TitleState::Working, 30);
    assert_eq!(claim.ledger().state, TurnState::Working);
    assert_eq!(claim.ledger().prompt_id, opened, "the answer resumes the turn");
}

#[test]
fn a_wait_that_opens_a_launch_holds_a_turn_of_its_own() {
    let mut claim = TitleClaim::default();
    claim.hear(TitleState::NeedsYou, 10);

    assert!(claim.answers(), "the first wait of a launch is its first event");
    assert_eq!(claim.ledger().state, TurnState::Blocked);
    assert!(claim.ledger().prompt_id.is_some(), "the wait belongs to a turn");
}

#[test]
fn a_plain_title_ends_the_turn_only_after_the_settle_time() {
    let mut claim = ready_claim();
    claim.hear(TitleState::Working, 10);
    claim.hear(TitleState::Plain, 20);

    assert!(!claim.settle(20 + TITLE_SETTLE_MILLIS - 1), "the title is not settled");
    assert_eq!(claim.ledger().state, TurnState::Working);
    assert!(claim.settle(20 + TITLE_SETTLE_MILLIS), "the plain title stands");
    assert_eq!(claim.ledger().state, TurnState::Finished);
    assert!(!claim.settle(20 + TITLE_SETTLE_MILLIS + 1), "the turn is closed once");
    claim.hear(TitleState::Plain, 1_000);
    assert!(!claim.settle(2_000), "a repeated plain title closes nothing");
    assert_eq!(claim.ledger().state, TurnState::Finished);
}

#[test]
fn a_plain_title_between_two_parts_of_a_turn_does_not_end_it() {
    let mut claim = ready_claim();
    claim.hear(TitleState::Working, 10);
    let opened = claim.ledger().prompt_id.clone();
    claim.hear(TitleState::Plain, 20);
    claim.hear(TitleState::Working, 120);

    assert!(!claim.settle(120 + TITLE_SETTLE_MILLIS));
    assert_eq!(claim.ledger().state, TurnState::Working);
    assert_eq!(claim.ledger().prompt_id, opened, "no new turn opened");
}

#[test]
fn a_plain_title_of_an_earlier_turn_cannot_close_the_turn_of_a_newer_one() {
    let mut claim = ready_claim();
    claim.hear(TitleState::Working, 10);
    claim.hear(TitleState::Plain, 20);
    assert!(claim.settle(20 + TITLE_SETTLE_MILLIS));
    let first = claim.ledger().prompt_id.clone();

    // The turn of the plain title ended, so the next working title of the launch
    // opens a turn of its own, and nothing of the old one closes it.
    claim.hear(TitleState::Working, 1_000);
    let second = claim.ledger().prompt_id.clone();
    assert_ne!(first, second, "a new turn of the same launch");
    assert_eq!(claim.ledger().state, TurnState::Working);
    assert!(
        !claim.settle(1_000 + TITLE_SETTLE_MILLIS),
        "no plain title of this turn stands"
    );

    // A settle that the same turn reports twice ends it once.
    claim.hear(TitleState::Plain, 2_000);
    assert!(claim.settle(2_000 + TITLE_SETTLE_MILLIS));
    assert!(!claim.settle(2_000 + TITLE_SETTLE_MILLIS));
    assert_eq!(claim.ledger().state, TurnState::Finished);
    assert_eq!(claim.ledger().prompt_id, second);
}

#[test]
fn the_start_up_of_a_launch_claims_no_turn() {
    let mut claim = TitleClaim::default();
    assert!(!claim.answers(), "no title of this launch has claimed a state");
    for frame in 0..8 {
        claim.hear(TitleState::Working, 100 + frame);
        assert!(!claim.ready(), "the program is loading");
        assert!(!claim.answers(), "the frames of the start-up claim nothing");
        assert_eq!(claim.ledger().state, TurnState::Unknown);
    }
    // The plain title is the moment the program is ready for the person, and the
    // spinner of a turn after it opens the first turn of the launch.
    claim.hear(TitleState::Plain, 900);
    assert!(claim.ready());
    assert!(!claim.answers(), "the ready title claims no turn");
    claim.hear(TitleState::Working, 1_000);
    assert!(claim.answers());
    assert_eq!(claim.ledger().state, TurnState::Working);
}

#[test]
fn a_question_of_a_launch_before_it_is_ready_claims_the_person() {
    let mut claim = TitleClaim::default();
    claim.hear(TitleState::Working, 100);
    claim.hear(TitleState::NeedsYou, 200);
    assert_eq!(claim.ledger().state, TurnState::Blocked, "the person must answer it");

    // The program is ready after the person answered, and the loading frames of
    // the launch after it claim nothing: no turn of theirs ran in the question, so
    // its end is no **Done**.
    claim.hear(TitleState::Working, 300);
    claim.hear(TitleState::Plain, 400);
    assert!(claim.ready());
    assert_eq!(claim.ledger().state, TurnState::Unknown);
    assert!(!claim.settle(400 + TITLE_SETTLE_MILLIS), "no turn of the person ran");
}
