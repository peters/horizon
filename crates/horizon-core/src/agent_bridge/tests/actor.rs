//! The actor of the orchestrator of one workspace: its build and its parse, and
//! the local ids that this form refuses.

use crate::agent_bridge::actor;

/// A local id of the form that `crate::new_local_id` makes.
const WORKSPACE: &str = "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e";

#[test]
fn the_actor_of_a_workspace_is_the_prefix_and_its_local_id() {
    assert_eq!(
        actor::of_workspace(WORKSPACE),
        Some(format!("horizon:orchestrator/{WORKSPACE}"))
    );
    assert_eq!(
        actor::workspace_of(&format!("horizon:orchestrator/{WORKSPACE}")),
        Some(WORKSPACE)
    );
}

#[test]
fn the_actor_of_a_workspace_differs_from_the_actor_of_a_panel() {
    let orchestrator = actor::of_workspace(WORKSPACE).expect("the actor of the workspace");
    let panel = crate::browser_actor(WORKSPACE);
    assert_ne!(orchestrator, panel);
    assert_eq!(actor::workspace_of(&panel), None);
    assert_eq!(panel, format!("horizon:{WORKSPACE}"));
}

#[test]
fn only_a_local_id_of_the_form_that_new_local_id_makes_is_accepted() {
    for refused in [
        "",
        "workspace",
        // A slash, a dot segment, a space and a control character.
        "8f14e45f/ceea-4a67-9f0b-3a1f2b3c4d5e",
        "..",
        "../../etc",
        "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e/x",
        "8f14e45f-ceea-4a67-9f0b",
        "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e0",
        // Upper-case letters, and a group of the wrong length.
        "8F14E45F-CEEA-4A67-9F0B-3A1F2B3C4D5E",
        "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5",
        "8f14e45fceea4a679f0b3a1f2b3c4d5e",
        "8f14e45f-ceea-4a67-9f0b-3a1f2b3c4d5e\n",
        // A name of a person, and a length above the bound.
        "peters",
        &"a".repeat(2 * actor::LOCAL_ID_BYTES).clone(),
    ] {
        assert_eq!(actor::of_workspace(refused), None, "the form refuses {refused:?}");
        assert!(!actor::is_local_id(refused), "the form refuses {refused:?}");
    }
    assert!(actor::is_local_id(WORKSPACE));
}

#[test]
fn every_other_actor_has_no_workspace() {
    for other in [
        "horizon:machine/build-1/night",
        "external",
        "horizon:orchestrator/",
        "horizon:orchestrator",
        "horizon:orchestrator/not-a-local-id",
        &format!("orchestrator/{WORKSPACE}"),
    ] {
        assert_eq!(actor::workspace_of(other), None, "{other:?} names no workspace");
    }
}

/// The front desk is one more actor of the dock: it belongs to no workspace, so
/// no local id names it and the two forms cannot meet.
#[test]
fn the_front_desk_is_one_actor_and_no_workspace() {
    assert_eq!(actor::FRONT_DESK, "horizon:front-desk");
    assert!(actor::is_front_desk(actor::FRONT_DESK));
    let orchestrator = actor::of_workspace(WORKSPACE).expect("the actor of the workspace");
    assert!(!actor::is_front_desk(&orchestrator));
    assert_eq!(
        actor::workspace_of(actor::FRONT_DESK),
        None,
        "the front desk owns no workspace"
    );
    assert!(!actor::is_local_id(actor::FRONT_DESK), "the front desk is no local id");
    assert!(
        !actor::is_uuid(actor::FRONT_DESK) && actor::FRONT_DESK.len() != actor::LOCAL_ID_BYTES,
        "a workspace local id is a UUID, so the reserved name never meets one"
    );
}
