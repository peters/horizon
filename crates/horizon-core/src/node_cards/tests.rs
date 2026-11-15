//! The socket paths and the actor that both sides of a card link agree on,
//! without a machine.

use std::path::Path;

use horizon_browser_control::manifest::block_machines;

use super::paths::MAX_SOCKET_PATH_BYTES;
use super::*;

#[test]
fn the_socket_path_of_a_session_holds_its_two_names() {
    assert_eq!(
        remote_socket("build-1", "fix-71").expect("a plain pair"),
        "/tmp/horizon-cards/build-1/fix-71.sock"
    );
    let directory = Path::new("/tmp/horizon-cards-test-1");
    assert_eq!(
        local_socket(directory, "build-1", "fix-71").expect("a plain pair"),
        directory.join("build-1").join("fix-71.sock")
    );
}

#[test]
fn a_name_outside_the_rule_makes_no_path_and_no_actor() {
    for (machine, session, code) in [
        ("../build", "fix", block_machines::INVALID_MACHINE_NAME),
        ("build-1", "fix 71", block_machines::INVALID_SESSION_NAME),
        ("build-1", "-fix", block_machines::INVALID_SESSION_NAME),
    ] {
        let refusal = remote_socket(machine, session).expect_err("a name outside the rule");
        assert_eq!(refusal.code, code);
        let refusal = actor(machine, session).expect_err("a name outside the rule");
        assert_eq!(refusal.code, code);
    }
    assert!(local_socket(Path::new("/tmp/x"), "build-1", "a/b").is_err());
}

#[test]
fn a_path_above_the_socket_bound_is_refused() {
    let deep = Path::new("/tmp").join("d".repeat(MAX_SOCKET_PATH_BYTES));
    let refusal = local_socket(&deep, "build-1", "fix-71").expect_err("a path above the bound");
    assert_eq!(refusal.code, SOCKET_PATH_TOO_LONG);
    assert!(refusal.message.contains("100"), "{}", refusal.message);
}

#[test]
fn the_actor_of_a_session_round_trips_and_names_no_local_panel() {
    let owner = actor("build-1", "fix-71").expect("a plain pair");
    assert_eq!(owner, "horizon:machine/build-1/fix-71");
    assert_eq!(actor_parts(&owner), Some(("build-1", "fix-71")));
    for actor in [
        "horizon:9f2a",
        "horizon:machine/build-1",
        "horizon:machine//fix",
        "machine/a/b",
    ] {
        assert_eq!(actor_parts(actor), None, "{actor}");
    }
}
