use super::*;

#[test]
fn unavailable_is_the_same_answer_for_every_hidden_panel() {
    let Outcome::Failed { code, .. } = unavailable() else {
        panic!("expected a failure");
    };
    assert_eq!(code, "panel_unavailable");
}

#[test]
fn a_target_stays_in_flight_while_a_submit_is_pending_and_for_a_moment_after() {
    let mut requests = AgentPanelRequests::default();
    let now = Instant::now();
    assert!(!requests.in_flight(PanelId(1), now));

    requests.note_sent(PanelId(1), now, true);
    assert!(
        requests.in_flight(PanelId(1), now),
        "its Enter has not been pressed yet"
    );
    assert!(!requests.in_flight(PanelId(2), now), "other agents are unaffected");

    requests.pending_submits.clear();
    assert!(requests.in_flight(PanelId(1), now + Duration::from_secs(1)));
    assert!(!requests.in_flight(PanelId(1), now + RECENT_SEND + Duration::from_secs(1)));
}

#[test]
fn a_send_without_enter_still_protects_the_prompt_for_a_moment() {
    let mut requests = AgentPanelRequests::default();
    let now = Instant::now();
    requests.note_sent(PanelId(7), now, false);
    assert!(requests.pending_submits.is_empty());
    assert!(requests.in_flight(PanelId(7), now + Duration::from_millis(500)));
}
