//! The command that opens the agent of a machine, and the session names that this
//! build opens and lists alone.

use crate::PanelKind;
use crate::machine_watch::{AgentState, MachineAgent};

/// The agent of the session `session` of one machine.
fn agent(session: &str) -> MachineAgent {
    MachineAgent {
        machine: "build-1".to_owned(),
        session: session.to_owned(),
        pane: "%0".to_owned(),
        kind: PanelKind::Codex,
        name: "shop".to_owned(),
        state: AgentState::Working,
    }
}

#[test]
fn an_agent_of_a_session_of_this_build_opens_that_session() {
    let command = agent("hz-abc123-1")
        .open_command()
        .expect("the session of the agent opens");
    assert_eq!(
        command,
        crate::machines::session_command("hz-abc123-1"),
        "the command is the one that a terminal of a machine builds"
    );
    assert_eq!(command, "tmux -u new-session -A -s hz-abc123-1 \\; set-option status off");
    assert!(agent("hz-abc123-1").can_open());
}

#[test]
fn a_session_that_a_person_made_by_hand_is_listed_and_cannot_be_opened() {
    for session in ["hz flat", "hz.one", "hz/one", "hz;one", "-hz-one", "", &"h".repeat(41)] {
        let refusal = agent(session)
            .open_command()
            .expect_err("the session of the test does not open");
        assert_eq!(
            refusal.code(),
            "invalid_session_name",
            "the refusal of {session:?} is the rule of a session name"
        );
        assert!(!agent(session).can_open(), "{session:?} cannot be opened");
    }
}

#[test]
fn the_longest_session_of_the_rule_still_opens() {
    let session = "h".repeat(40);
    assert!(
        agent(&session)
            .open_command()
            .expect("the longest session of the rule opens")
            .contains(&session)
    );
    assert!(agent(&session).can_open());
}

#[test]
fn the_rule_of_an_open_command_is_the_rule_of_the_machine_list() {
    for session in ["hz-1", "hz_1", "hz-1.a", "hz 1", "-hz"] {
        assert_eq!(
            agent(session).can_open(),
            crate::machines::session_name(session).is_ok(),
            "the open command and the machine list share one rule of a session name: {session:?}"
        );
    }
}
