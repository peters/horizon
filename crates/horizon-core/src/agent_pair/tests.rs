use super::{AgentPairQueue, AgentPairRole, FindingStatus, RegressionEvidencePacket};

fn candidate_queue() -> (AgentPairQueue, String) {
    let mut queue = AgentPairQueue::new();
    let id = queue
        .create_candidate(
            "Crash on resize",
            "Detached window snaps back during resize.",
            "Observed in a native window trace.",
            vec!["crates/horizon-ui/src/app/detached_viewports.rs".to_string()],
            vec!["cargo test --workspace".to_string()],
        )
        .expect("candidate");
    (queue, id)
}

fn complete_packet() -> RegressionEvidencePacket {
    RegressionEvidencePacket {
        verification_summary: "Confirmed the resize path and fixed restore replay.".to_string(),
        validation_commands: vec![
            "cargo test --workspace".to_string(),
            "cargo clippy --all-targets --all-features -- -D warnings".to_string(),
        ],
        validation_result: "All validation passed.".to_string(),
        regression_scope: "Resize, relaunch restore, and detached focus paths.".to_string(),
    }
}

#[test]
fn candidate_can_become_accepted() {
    let (mut queue, id) = candidate_queue();

    queue.accept_candidate(&id).expect("accept");

    assert_eq!(queue.card(&id).expect("card").status, FindingStatus::Accepted);
}

#[test]
fn candidate_can_become_rejected() {
    let (mut queue, id) = candidate_queue();

    queue.reject_candidate(&id).expect("reject");

    assert_eq!(queue.card(&id).expect("card").status, FindingStatus::Rejected);
}

#[test]
fn accepted_can_dispatch_to_linked_performer() {
    let (mut queue, id) = candidate_queue();
    queue
        .link_panel(AgentPairRole::Performer, "performer-panel-local-id")
        .expect("link performer");
    queue.accept_candidate(&id).expect("accept");

    let prompt = queue.dispatch_to_performer(&id).expect("dispatch");

    let card = queue.card(&id).expect("card");
    assert_eq!(card.status, FindingStatus::Implementing);
    assert_eq!(
        card.assigned_performer_panel_local_id.as_deref(),
        Some("performer-panel-local-id")
    );
    assert!(prompt.contains(&format!("Implement accepted finding {id}")));
}

#[test]
fn candidate_rejected_and_verified_cards_cannot_dispatch() {
    let (mut candidate, candidate_id) = candidate_queue();
    candidate
        .link_panel(AgentPairRole::Performer, "performer")
        .expect("link performer");
    assert!(candidate.dispatch_to_performer(&candidate_id).is_err());
    assert_eq!(
        candidate.card(&candidate_id).expect("candidate").status,
        FindingStatus::Candidate
    );

    let (mut rejected, rejected_id) = candidate_queue();
    rejected
        .link_panel(AgentPairRole::Performer, "performer")
        .expect("link performer");
    rejected.reject_candidate(&rejected_id).expect("reject");
    assert!(rejected.dispatch_to_performer(&rejected_id).is_err());

    let (mut verified, verified_id) = candidate_queue();
    verified
        .link_panel(AgentPairRole::Performer, "performer")
        .expect("link performer");
    verified.accept_candidate(&verified_id).expect("accept");
    verified.dispatch_to_performer(&verified_id).expect("dispatch");
    verified
        .verify_with_evidence(&verified_id, complete_packet())
        .expect("verify");
    assert!(verified.dispatch_to_performer(&verified_id).is_err());
}

#[test]
fn implementing_can_become_verified_with_complete_evidence() {
    let (mut queue, id) = candidate_queue();
    queue
        .link_panel(AgentPairRole::Performer, "performer")
        .expect("link performer");
    queue.accept_candidate(&id).expect("accept");
    queue.dispatch_to_performer(&id).expect("dispatch");

    queue.verify_with_evidence(&id, complete_packet()).expect("verify");

    let card = queue.card(&id).expect("card");
    assert_eq!(card.status, FindingStatus::Verified);
    assert!(card.regression_evidence.is_some());
}

#[test]
fn incomplete_evidence_is_rejected() {
    let (mut queue, id) = candidate_queue();
    queue
        .link_panel(AgentPairRole::Performer, "performer")
        .expect("link performer");
    queue.accept_candidate(&id).expect("accept");
    queue.dispatch_to_performer(&id).expect("dispatch");

    let packet = RegressionEvidencePacket {
        verification_summary: "Confirmed".to_string(),
        validation_commands: vec![],
        validation_result: "Passed".to_string(),
        regression_scope: "Queue only".to_string(),
    };

    assert!(queue.verify_with_evidence(&id, packet).is_err());
    assert_eq!(queue.card(&id).expect("card").status, FindingStatus::Implementing);
}

#[test]
fn prompt_generation_includes_structured_finding_fields() {
    let (queue, id) = candidate_queue();
    let card = queue.card(&id).expect("card");

    let prompt = card.performer_prompt();

    assert!(prompt.contains(&id));
    assert!(prompt.contains("Crash on resize"));
    assert!(prompt.contains("Detached window snaps back"));
    assert!(prompt.contains("Observed in a native window trace"));
    assert!(prompt.contains("crates/horizon-ui/src/app/detached_viewports.rs"));
    assert!(prompt.contains("cargo test --workspace"));
}

#[test]
fn panel_links_use_stable_panel_local_ids() {
    let mut queue = AgentPairQueue::new();

    queue
        .link_panel(AgentPairRole::Researcher, "researcher-local-id")
        .expect("link researcher");
    queue
        .link_panel(AgentPairRole::Performer, "performer-local-id")
        .expect("link performer");

    assert_eq!(
        queue
            .link_for(AgentPairRole::Researcher)
            .expect("researcher")
            .panel_local_id,
        "researcher-local-id"
    );
    assert_eq!(
        queue
            .link_for(AgentPairRole::Performer)
            .expect("performer")
            .panel_local_id,
        "performer-local-id"
    );
}
