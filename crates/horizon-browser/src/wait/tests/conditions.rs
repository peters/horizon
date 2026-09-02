use super::*;

#[test]
fn a_wait_settles_when_the_selector_reaches_the_requested_state() {
    let now = Instant::now();
    let mut wait = pending(SelectorState::Visible, 5_000, now);
    assert!(wait.poll_due(now));
    assert!(
        matches!(wait.observe(7, &[], None, now), Observation::Waiting),
        "no match keeps waiting"
    );
    assert!(
        !wait.poll_due(now + Duration::from_millis(50)),
        "observations are paced"
    );
    assert!(wait.poll_due(now + WAIT_POLL_INTERVAL));
    assert!(
        matches!(
            wait.observe(7, &[node(false)], None, now + Duration::from_millis(100)),
            Observation::Waiting
        ),
        "a hidden match does not satisfy visible"
    );
    assert!(matches!(
        wait.observe(7, &[node(true)], None, now + Duration::from_millis(1_500)),
        Observation::Satisfied
    ));
    // Released later, after the coordination refresh: the elapsed time
    // still ends at the observation that met the condition.
    let satisfied = released(&wait, vec![node(true)], now + Duration::from_millis(1_900));
    assert_eq!(satisfied.state, SelectorState::Visible);
    assert_eq!(satisfied.polls, 3);
    assert_eq!(satisfied.elapsed_millis, 1_500);
    assert_eq!(satisfied.nodes.len(), 1);

    let mut hidden = pending(SelectorState::Hidden, 5_000, now);
    assert!(matches!(
        hidden.observe(7, &[node(true)], None, now),
        Observation::Waiting
    ));
    assert!(
        matches!(hidden.observe(7, &[], None, now), Observation::Satisfied),
        "an empty match set is hidden"
    );

    let mut present = pending(SelectorState::Present, 5_000, now);
    assert!(matches!(
        present.observe(7, &[node(false)], None, now),
        Observation::Satisfied
    ));
    assert_eq!(released(&present, vec![node(false)], now).polls, 1);
}

#[test]
fn the_condition_counts_every_match_and_the_outcome_is_capped() {
    let now = Instant::now();
    let mut nodes: Vec<BrowserNode> = (0..25).map(|_| node(false)).collect();
    nodes.push(node(true));
    let mut visible = pending(SelectorState::Visible, 1_000, now);
    assert!(
        matches!(visible.observe(7, &nodes, None, now), Observation::Satisfied),
        "a visible match beyond the cap counts"
    );
    assert_eq!(
        released(&visible, nodes.clone(), now).nodes.len(),
        WAIT_MAX_RESULTS as usize
    );
    let mut hidden = pending(SelectorState::Hidden, 1_000, now);
    assert!(
        matches!(hidden.observe(7, &nodes, None, now), Observation::Waiting),
        "a visible match beyond the cap keeps hidden unsatisfied"
    );

    // With the scan's summary, matches beyond the returned cap decide too:
    // 300 matches of which only the 251st is visible.
    let beyond = ScanSummary {
        matched: 300,
        visible: 1,
    };
    let capped: Vec<BrowserNode> = (0..250).map(|_| node(false)).collect();
    let mut visible_beyond = pending(SelectorState::Visible, 1_000, now);
    assert!(
        matches!(
            visible_beyond.observe(7, &capped, Some(beyond), now),
            Observation::Satisfied
        ),
        "a visible match beyond the scan cap satisfies visible"
    );
    let mut hidden_beyond = pending(SelectorState::Hidden, 1_000, now);
    assert!(
        matches!(
            hidden_beyond.observe(7, &capped, Some(beyond), now),
            Observation::Waiting
        ),
        "a visible match beyond the scan cap keeps hidden unsatisfied"
    );
    let mut present_none = pending(SelectorState::Present, 1_000, now);
    assert!(matches!(
        present_none.observe(7, &[], Some(ScanSummary { matched: 0, visible: 0 }), now),
        Observation::Waiting
    ));
}
