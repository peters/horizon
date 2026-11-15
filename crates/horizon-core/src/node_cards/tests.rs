//! The socket paths, the actor and the entry texts that both sides of a card
//! link agree on, without a machine.

use std::path::Path;

use horizon_browser_control::manifest::block_machines;

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

#[test]
fn each_relay_program_gets_its_own_command_over_one_socket() {
    let socat = relay_command("build-1", RelayProgram::Socat).expect("a machine name");
    assert_eq!(socat.command, "sh");
    let line = socat.args.last().expect("the shell line");
    assert!(line.contains("UNIX-CONNECT:\"$d/$s.sock\""), "{line}");
    let nc = relay_command("build-1", RelayProgram::Nc).expect("a machine name");
    assert!(
        nc.args
            .last()
            .expect("the shell line")
            .contains("exec nc -U \"$d/$s.sock\"")
    );
    let python = relay_command("build-1", RelayProgram::Python).expect("a machine name");
    let line = python.args.last().expect("the shell line");
    assert!(line.contains(PYTHON_RELAY), "{line}");
    assert!(
        !PYTHON_RELAY.contains('\''),
        "the script passes a shell in double quotes"
    );
    for relay in [&socat, &nc, &python] {
        let line = relay.args.last().expect("the shell line");
        assert_eq!(relay.command, "sh");
        assert_eq!(relay.args.first().map(String::as_str), Some("-c"));
        assert!(line.starts_with("d=/tmp/horizon-cards/build-1;"), "{line}");
        assert!(line.contains("tmux display-message -p '#S'"), "{line}");
        assert!(line.contains("chmod 700"), "{line}");
    }
}

#[test]
fn the_probe_answers_the_first_program_of_its_three() {
    assert_eq!(probe_outcome("/usr/bin/socat\n"), Some(RelayProgram::Socat));
    assert_eq!(probe_outcome("/usr/bin/nc\n/usr/bin/python3\n"), Some(RelayProgram::Nc));
    assert_eq!(probe_outcome("/usr/bin/python3\n"), Some(RelayProgram::Python));
    assert_eq!(probe_outcome(""), None);
    assert_eq!(probe_outcome("/usr/bin/sudo\n"), None);
}

#[test]
fn a_writer_gets_the_words_of_its_own_program() {
    let relay = relay_command("build-1", RelayProgram::Socat).expect("a machine name");
    let args = relay.args.clone();
    for (agent, head) in [
        (
            CardAgent::Claude,
            vec!["mcp", "add", "--scope", "user", ENTRY_NAME, "--", "sh"],
        ),
        (CardAgent::Codex, vec!["mcp", "add", ENTRY_NAME, "--", "sh"]),
        (CardAgent::Pi, vec!["mcp", "add", ENTRY_NAME, "--", "sh"]),
    ] {
        let words = agent.writer_args(&relay.command, &args);
        assert_eq!(&words[..head.len()], head.as_slice(), "{}", agent.program());
        assert_eq!(&words[head.len()..], args.as_slice());
    }
}

#[test]
fn a_merge_names_the_same_command_as_json_and_as_toml() {
    let relay = relay_command("build-1", RelayProgram::Nc).expect("a machine name");
    let entry = entry_json(&relay.command, &relay.args);
    assert_eq!(entry["command"], "sh");
    assert_eq!(entry["args"][0], "-c");
    let table = entry_toml(&relay.command, &relay.args);
    assert!(table.starts_with("[mcp_servers.horizon-cards]\n"), "{table}");
    assert!(table.contains("command = \"sh\""), "{table}");
    assert_eq!(entry_file(CardAgent::Pi), "~/.pi/agent/mcp.json");
    assert_eq!(entry_file(CardAgent::Claude), "~/.claude.json");
    assert_eq!(entry_file(CardAgent::Codex), "~/.codex/config.toml");
}
