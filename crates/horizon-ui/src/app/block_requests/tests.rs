//! The card operations a frame applies, judged on a test board.

use std::time::Duration;

use super::*;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};
use horizon_core::{RuntimeState, StartupDecision};

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![
            editor_workspace_state("first", [0.0, 0.0]),
            editor_workspace_state("second", [900.0, 0.0]),
        ],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    // No live agent process is needed to exercise the caller's identity.
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    (temp, ctx, app)
}

fn document(heading: &str) -> serde_json::Value {
    serde_json::json!({
        "components": [
            {"type": "heading", "text": heading},
            {"type": "buttons", "items": [{"id": "stop", "label": "Stop", "tone": "warn"}]}
        ]
    })
}

fn request_from(app: &HorizonApp, panel: usize, operation: Operation) -> Request {
    Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[panel].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 10_000,
        operation,
    }
}

fn request(app: &HorizonApp, operation: Operation) -> Request {
    request_from(app, 0, operation)
}

/// Puts the second agent panel in the first workspace, so two agents share it.
fn share_workspace(app: &mut HorizonApp) {
    app.board.panels[1].workspace_id = app.board.panels[0].workspace_id;
}

fn created(outcome: Outcome) -> (String, u64) {
    match outcome {
        Outcome::Created { panel_id, revision } => (panel_id, revision),
        other => panic!("expected a created card: {other:?}"),
    }
}

fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn a_created_card_lands_in_the_callers_workspace_without_taking_focus() {
    let (_temp, ctx, mut app) = app();
    let workspace = app.board.panels[0].workspace_id;
    let other_workspace = app.board.panels[1].workspace_id;
    app.board.focus(app.board.panels[1].id);
    let before = app.board.panels.len();

    let (panel_id, revision) = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: Some("cloud-1".into()),
                document: document("cloud-1"),
            },
        ),
        &ctx,
    ));

    assert_eq!(revision, 1);
    assert_eq!(app.board.panels.len(), before + 1);
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("card"))
        .expect("card");
    assert_eq!(panel.workspace_id, workspace);
    assert_ne!(panel.workspace_id, other_workspace);
    assert_eq!(panel.kind, PanelKind::Block);
    let state = panel.block().expect("card state");
    assert_eq!(state.title, "cloud-1");
    assert_eq!(state.revision, 1);
    assert_eq!(
        state.owner_actor.as_deref(),
        Some(format!("horizon:{}", app.board.panels[0].local_id).as_str())
    );
    assert!(panel.visible, "a new card is visible");
    assert_eq!(
        app.board.focused,
        Some(app.board.panels[1].id),
        "creating a card must not steal keyboard input"
    );
}

#[test]
fn only_the_owner_updates_visibility_and_close() {
    let (_temp, ctx, mut app) = app();
    share_workspace(&mut app);
    let (panel_id, _) = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: document("owner's card"),
            },
        ),
        &ctx,
    ));

    for operation in [
        Operation::Update {
            panel_id: panel_id.clone(),
            document: document("intruder"),
        },
        Operation::Visibility {
            panel_id: panel_id.clone(),
            visible: false,
        },
        Operation::Close {
            panel_id: panel_id.clone(),
        },
    ] {
        let refusal = app.apply_block_request(&request_from(&app, 1, operation), &ctx);
        assert_eq!(code(&refusal), "not_owner");
    }
    assert_eq!(
        app.workspace_cards(app.board.panels[0].workspace_id).len(),
        1,
        "a refused request changes no panel"
    );

    let updated = app.apply_block_request(
        &request(
            &app,
            Operation::Update {
                panel_id: panel_id.clone(),
                document: document("second"),
            },
        ),
        &ctx,
    );
    assert!(matches!(updated, Outcome::Updated { revision: 2, .. }), "{updated:?}");
    let hidden = app.apply_block_request(
        &request(
            &app,
            Operation::Visibility {
                panel_id: panel_id.clone(),
                visible: false,
            },
        ),
        &ctx,
    );
    assert!(matches!(hidden, Outcome::Visibility { visible: false, .. }));
    let id = app.board.panel_id_by_local_id(&panel_id).expect("card");
    assert!(!app.board.panel(id).expect("card").visible);

    let closed = app.apply_block_request(
        &request(
            &app,
            Operation::Close {
                panel_id: panel_id.clone(),
            },
        ),
        &ctx,
    );
    assert!(matches!(closed, Outcome::Closed { .. }), "{closed:?}");
    assert!(app.board.panel_id_by_local_id(&panel_id).is_none());
}

#[test]
fn an_agent_owns_at_most_eight_cards_in_one_workspace() {
    let (_temp, ctx, mut app) = app();
    for index in 0..block::MAX_CARDS_PER_OWNER {
        let (_, revision) = created(app.apply_block_request(
            &request(
                &app,
                Operation::Create {
                    title: Some(format!("card {index}")),
                    document: document(&format!("card {index}")),
                },
            ),
            &ctx,
        ));
        assert_eq!(revision, 1);
    }
    let refusal = app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: document("ninth"),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&refusal), "too_many_cards");

    // Another agent of the same workspace has its own allowance.
    let other = app.apply_block_request(
        &request_from(
            &app,
            1,
            Operation::Create {
                title: None,
                document: document("other agent"),
            },
        ),
        &ctx,
    );
    assert!(matches!(other, Outcome::Created { .. }), "{other:?}");
}

#[test]
fn a_card_outside_the_callers_workspace_or_a_panel_that_is_not_a_card_is_refused() {
    let (_temp, ctx, mut app) = app();
    let (panel_id, _) = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: document("first workspace"),
            },
        ),
        &ctx,
    ));
    let inspect = Operation::Inspect {
        panel_id: panel_id.clone(),
    };
    // The second agent panel is in another workspace.
    let outside = app.apply_block_request(&request_from(&app, 1, inspect.clone()), &ctx);
    assert_eq!(code(&outside), "panel_unavailable");

    let agent_panel = app.board.panels[0].local_id.clone();
    let not_card = app.apply_block_request(
        &request(
            &app,
            Operation::Inspect {
                panel_id: agent_panel.clone(),
            },
        ),
        &ctx,
    );
    assert_eq!(code(&not_card), "not_block_panel");
    for operation in [
        Operation::Update {
            panel_id: agent_panel.clone(),
            document: document("x"),
        },
        Operation::Visibility {
            panel_id: agent_panel.clone(),
            visible: false,
        },
        Operation::Close { panel_id: agent_panel },
    ] {
        assert_eq!(
            code(&app.apply_block_request(&request(&app, operation), &ctx)),
            "not_block_panel"
        );
    }
}

#[test]
fn a_document_that_breaks_a_rule_names_the_offending_place() {
    let (_temp, ctx, mut app) = app();
    let before = app.board.panels.len();
    let invalid = serde_json::json!({
        "components": [{"type": "buttons", "items": [{"id": "Not An Id", "label": "Stop"}]}]
    });
    let refusal = app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: invalid,
            },
        ),
        &ctx,
    );
    let Outcome::Failed { code: failure, message } = &refusal else {
        panic!("expected a refusal: {refusal:?}");
    };
    assert_eq!(failure, "invalid_document");
    assert!(message.contains("components[0].items[0].id"), "{message}");
    assert_eq!(app.board.panels.len(), before, "a refused document creates no card");

    let huge = serde_json::json!({
        "components": [{"type": "text", "text": "x".repeat(horizon_browser_control::block::BlockDocument::MAX_SERIALIZED_BYTES)}]
    });
    let too_large = app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: huge,
            },
        ),
        &ctx,
    );
    assert_eq!(code(&too_large), "document_too_large");
}

#[test]
fn list_and_inspect_report_the_cards_of_the_callers_workspace() {
    let (_temp, ctx, mut app) = app();
    share_workspace(&mut app);
    let mine = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: Some("mine".into()),
                document: document("mine"),
            },
        ),
        &ctx,
    ))
    .0;
    let theirs = created(app.apply_block_request(
        &request_from(
            &app,
            1,
            Operation::Create {
                title: Some("theirs".into()),
                document: document("theirs"),
            },
        ),
        &ctx,
    ))
    .0;

    let Outcome::Panels { panels } = app.apply_block_request(&request(&app, Operation::List), &ctx) else {
        panic!("expected a list");
    };
    let listed: Vec<_> = panels
        .iter()
        .map(|panel| (panel.panel_id.clone(), panel.title.clone(), panel.owned_by_caller))
        .collect();
    assert!(listed.contains(&(mine.clone(), "mine".to_owned(), true)), "{listed:?}");
    assert!(
        listed.contains(&(theirs.clone(), "theirs".to_owned(), false)),
        "{listed:?}"
    );

    let Outcome::Inspected {
        title,
        revision,
        document: reported,
        visible,
        agent_present,
        owned_by_caller,
        ..
    } = app.apply_block_request(&request(&app, Operation::Inspect { panel_id: mine.clone() }), &ctx)
    else {
        panic!("expected an inspection");
    };
    assert_eq!((title.as_str(), revision, visible), ("mine", 1, true));
    assert_eq!(reported, card_document(&document("mine")).expect("document"));
    assert!(agent_present, "the owning agent is in this workspace");
    assert!(owned_by_caller);

    // A card whose owner has left the workspace reports no agent.
    let id = app.board.panel_id_by_local_id(&theirs).expect("card");
    app.board
        .panel_mut(id)
        .and_then(Panel::block_mut)
        .expect("card state")
        .owner_actor = Some("horizon:gone".into());
    let Outcome::Inspected {
        agent_present,
        owned_by_caller,
        ..
    } = app.apply_block_request(
        &request(
            &app,
            Operation::Inspect {
                panel_id: theirs.clone(),
            },
        ),
        &ctx,
    )
    else {
        panic!("expected an inspection");
    };
    assert!(!agent_present);
    assert!(!owned_by_caller);
}

#[test]
fn every_update_raises_the_revision_and_replaces_the_whole_document() {
    let (_temp, ctx, mut app) = app();
    let (panel_id, _) = created(app.apply_block_request(
        &request(
            &app,
            Operation::Create {
                title: None,
                document: document("first"),
            },
        ),
        &ctx,
    ));
    for (index, heading) in ["second", "third"].iter().enumerate() {
        let outcome = app.apply_block_request(
            &request(
                &app,
                Operation::Update {
                    panel_id: panel_id.clone(),
                    document: document(heading),
                },
            ),
            &ctx,
        );
        let expected = 2 + u64::try_from(index).expect("small index");
        assert!(
            matches!(outcome, Outcome::Updated { revision, .. } if revision == expected),
            "{outcome:?}"
        );
    }
    let id = app.board.panel_id_by_local_id(&panel_id).expect("card");
    let state = app.board.panel(id).and_then(Panel::block).expect("card state");
    assert_eq!(state.revision, 3);
    assert_eq!(
        state.document.components,
        card_document(&document("third")).expect("document").components
    );
}

#[test]
fn another_host_an_expired_deadline_and_a_missing_caller_are_refused() {
    let (_temp, ctx, mut app) = app();
    let mut wrong_host = request(&app, Operation::List);
    wrong_host.host_instance = "another-host".into();
    assert_eq!(code(&app.apply_block_request(&wrong_host, &ctx)), "wrong_host");

    let mut expired = request(&app, Operation::List);
    expired.deadline_at_millis = manifest::now_millis() - 1;
    assert_eq!(code(&app.apply_block_request(&expired, &ctx)), "request_expired");

    let mut gone = request(
        &app,
        Operation::Create {
            title: None,
            document: document("gone"),
        },
    );
    gone.actor = "horizon:gone".into();
    assert_eq!(code(&app.apply_block_request(&gone, &ctx)), "caller_unavailable");
}

#[test]
fn a_queued_card_request_is_claimed_applied_and_answered() {
    let root = tempfile::tempdir().expect("runtime root");
    let (_temp, ctx, mut app) = app();
    let actor = format!("horizon:{}", app.board.panels[0].local_id);
    let queued = block::enqueue_at(
        root.path(),
        manifest::AgentIdentity::new(&actor, Some(manifest::host_instance())),
        Operation::Create {
            title: Some("queued".into()),
            document: document("queued"),
        },
        Duration::from_secs(10),
    )
    .expect("enqueue");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        app.claim_block_requests(Some(root.path()));
        app.settle_browser_host_io();
        app.drain_claimed_block_requests(&ctx);
        app.settle_browser_host_io();
        if let Some(outcome) = block::take_result_at(root.path(), &queued).expect("result") {
            break outcome;
        }
        assert!(std::time::Instant::now() < deadline, "the request was never answered");
        std::thread::sleep(Duration::from_millis(5));
    };
    let (panel_id, revision) = created(outcome);
    assert_eq!(revision, 1);
    assert!(app.board.panel_id_by_local_id(&panel_id).is_some());
    assert!(!app.block_claims_outstanding(), "no claimed request is left unanswered");
}
