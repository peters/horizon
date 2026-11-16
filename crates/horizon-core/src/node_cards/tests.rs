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

/// One machine record of a test, with the port and the name it holds.
fn record(name: &str) -> crate::Machine {
    crate::Machine {
        name: name.to_owned(),
        host: "127.0.0.1".to_owned(),
        user: None,
        port: Some(2222),
        session: None,
        provider: None,
        repository: None,
        folder: None,
        added_at_millis: 0,
        cards: true,
    }
}

/// The login of a machine terminal of `name`, in a folder of the test.
fn login(folder: &Path, name: &str) -> crate::SshConnection {
    let registry = crate::MachineRegistry::new(folder.to_path_buf());
    let machines = crate::Machines::with_records(registry, vec![record(name)]);
    let keys = crate::MachineKeys::new(folder.join("machine-keys"));
    crate::machine_watch::connection(&machines, &keys, name).expect("the login of a machine terminal")
}

/// The two socket paths of `session` of `build-1`, in a folder of the test.
fn sockets(directory: &Path, sessions: &[&str]) -> Vec<(String, std::path::PathBuf)> {
    sessions
        .iter()
        .map(|session| {
            (
                remote_socket("build-1", session).expect("a plain pair"),
                local_socket(directory, "build-1", session).expect("a plain pair"),
            )
        })
        .collect()
}

#[test]
fn the_child_of_one_session_forwards_one_socket_and_holds_no_terminal() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let connection = login(folder.path(), "build-1");
    let args = forward_args(&connection, &sockets(Path::new("/tmp/hz-cards-1"), &["fix-71"]));
    assert_eq!(args.first().map(String::as_str), Some("-N"));
    assert_eq!(args.get(1).map(String::as_str), Some("-T"));
    assert!(
        !args.iter().any(|argument| argument == "-t" || argument == "-L"),
        "no terminal and no local forward: {args:?}"
    );
    let forwarded: Vec<&String> = args
        .iter()
        .skip_while(|argument| argument.as_str() != "-R")
        .skip(1)
        .take_while(|argument| argument.as_str() != "-o")
        .collect();
    assert_eq!(forwarded.len(), 1, "{args:?}");
    assert_eq!(
        forwarded[0].as_str(),
        "/tmp/horizon-cards/build-1/fix-71.sock:/tmp/hz-cards-1/build-1/fix-71.sock"
    );
    for option in ["ExitOnForwardFailure=yes", "StreamLocalBindUnlink=yes", "BatchMode=yes"] {
        assert!(args.iter().any(|argument| argument == option), "{args:?}");
    }
}

#[test]
fn the_prune_of_a_machine_makes_the_folder_of_its_sockets_before_it_removes_them() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let connection = login(folder.path(), "build-1");
    let folders = "/tmp/horizon-cards /tmp/horizon-cards/build-1";
    for (sessions, sockets) in [
        (&["fix-71"][..], "/tmp/horizon-cards/build-1/fix-71.sock"),
        (
            &["fix-71", "night"][..],
            "/tmp/horizon-cards/build-1/fix-71.sock /tmp/horizon-cards/build-1/night.sock",
        ),
    ] {
        let remotes: Vec<String> = sessions
            .iter()
            .map(|session| remote_socket("build-1", session).expect("a plain pair"))
            .collect();
        let args = prune_args(&connection, &remotes);
        let words = format!("mkdir -p {folders} && chmod 700 {folders} && rm -f -- {sockets}");
        assert_eq!(args.last().map(String::as_str), Some(words.as_str()), "of {sessions:?}");
    }
}

#[test]
fn three_sessions_of_one_machine_get_three_forwards_of_one_child() {
    let folder = tempfile::tempdir().expect("a folder of the test");
    let connection = login(folder.path(), "build-1");
    let args = forward_args(
        &connection,
        &sockets(Path::new("/tmp/hz-cards-2"), &["fix-71", "night", "plan"]),
    );
    let forwards = args.iter().filter(|argument| argument.as_str() == "-R").count();
    assert_eq!(forwards, 3, "{args:?}");
    for session in ["fix-71", "night", "plan"] {
        assert!(
            args.iter().any(|argument| argument
                == &format!("/tmp/horizon-cards/build-1/{session}.sock:/tmp/hz-cards-2/build-1/{session}.sock")),
            "{session} of {args:?}"
        );
    }
    // The login of a machine terminal ends the argv, after the forwards.
    assert_eq!(args.last().map(String::as_str), Some("127.0.0.1"));
}
