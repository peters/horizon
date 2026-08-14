use super::super::truncate_session_label;

#[test]
fn truncation_trims_and_honors_the_character_budget() {
    let exact_budget = format!("  {}  ", "å".repeat(64));
    let over_budget = format!("  {}  ", "å".repeat(65));

    assert_eq!(truncate_session_label(&exact_budget), "å".repeat(64));
    assert_eq!(truncate_session_label(&over_budget), format!("{}…", "å".repeat(63)));
}
