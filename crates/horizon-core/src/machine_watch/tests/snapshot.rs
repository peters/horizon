//! The snapshot of the watch: its version, its totals and the rows of its
//! machines, which a frame reads.

use crate::PanelKind;
use crate::machine_watch::{
    AgentPrograms, AgentState, MachineWatchSnapshot, Shared, WATCH_FORMAT_VERSION, WatchLink, WatchRefusal,
    parse_report,
};

/// One report of one machine with the panes of `panes`.
fn report(panes: &[(&str, &str, &str)]) -> crate::machine_watch::Report {
    let mut lines = vec![format!("HZ\t{WATCH_FORMAT_VERSION}").into_bytes()];
    for (pane, command, title) in panes {
        lines.push(format!("P\t{pane}\thz-1\t{command}\t/home/person/shop\t{title}").into_bytes());
    }
    lines.push(b"Z".to_vec());
    parse_report(&lines).expect("the report of the test parses")
}

#[test]
fn the_version_of_the_snapshot_rises_only_when_a_row_changed() {
    let shared = Shared::new();
    shared.set_link("build-1", WatchLink::Connecting);
    let first = shared.snapshot();
    assert_eq!(first.version, 1);
    let again = shared.snapshot();
    assert_eq!(again.version, 1, "a link that did not change does no work");
    shared.set_link("build-1", WatchLink::Connecting);
    assert_eq!(shared.snapshot().version, 1, "a repeated link is no change");

    shared.apply_report("build-1", &report(&[("%0", "codex", "shop")]));
    let working = shared.snapshot();
    assert!(working.version > first.version, "the report changed a row");
    assert_eq!(
        working.machine("build-1").map(|state| state.link),
        Some(WatchLink::Watching)
    );
    shared.apply_report("build-1", &report(&[("%0", "codex", "shop")]));
    assert_eq!(
        shared.snapshot().version,
        working.version,
        "the same report changes no row and no frame"
    );
}

#[test]
fn the_totals_of_the_snapshot_count_the_agents_that_wait_for_the_person() {
    let shared = Shared::new();
    shared.apply_report(
        "build-1",
        &report(&[
            ("%0", "codex", "\u{280b} shop"),
            ("%1", "bash", "shop"),
            ("%2", "claude", "shop"),
        ]),
    );
    let snapshot = shared.snapshot();
    assert_eq!(snapshot.agents("build-1").len(), 2, "the shell pane is no agent");
    assert_eq!(snapshot.needs_you, 0);
    assert_eq!(snapshot.done, 0);
    let codex = &snapshot.agents("build-1")[0];
    assert_eq!(codex.kind, PanelKind::Codex);
    assert_eq!(codex.state, AgentState::Working);

    // The turn of the Codex agent ends: the row waits for the person.
    shared.apply_report("build-1", &report(&[("%0", "codex", "shop"), ("%2", "claude", "shop")]));
    let snapshot = shared.snapshot();
    assert_eq!(snapshot.done, 1, "a turn that ended unseen is Done");
    assert_eq!(snapshot.needs_you, 0);
    assert_eq!(
        snapshot.agents("build-1").iter().map(|agent| agent.state).max(),
        Some(AgentState::Done),
        "the heaviest state of the machine is Done"
    );
    assert!(shared.mark_seen("build-1", "hz-1", "%0"), "the look clears the row");
    let snapshot = shared.snapshot();
    assert_eq!(snapshot.done, 0);
    assert_eq!(snapshot.agents("build-1")[0].state, AgentState::Idle);
    let version = snapshot.version;
    assert!(
        !shared.mark_seen("build-1", "hz-1", "%0"),
        "a second look changes nothing"
    );
    assert_eq!(shared.snapshot().version, version);
    assert!(
        !shared.mark_seen("build-1", "hz-9", "%0"),
        "an unknown pane changes nothing"
    );
    assert_eq!(shared.snapshot().version, version);
}

#[test]
fn a_note_of_the_machine_is_the_link_of_its_snapshot() {
    let shared = Shared::new();
    let note = parse_report(&[b"HZ\t1".to_vec(), b"X\tno-server".to_vec(), b"Z".to_vec()]).expect("the note parses");
    shared.apply_report("build-1", &note);
    assert_eq!(
        shared.snapshot().machine("build-1").map(|state| state.link),
        Some(WatchLink::NoServer)
    );
    let note = parse_report(&[b"HZ\t1".to_vec(), b"X\tno-tmux".to_vec(), b"Z".to_vec()]).expect("the note parses");
    shared.apply_report("build-1", &note);
    assert_eq!(
        shared.snapshot().machine("build-1").map(|state| state.link),
        Some(WatchLink::NoTmux)
    );
}

#[test]
fn a_link_that_goes_down_leaves_no_row_of_an_older_report() {
    let shared = Shared::new();
    shared.apply_report("build-1", &report(&[("%0", "codex", "shop")]));
    assert_eq!(shared.snapshot().agents("build-1").len(), 1);
    shared.set_link(
        "build-1",
        WatchLink::Lost {
            since_millis: 1_000,
            attempt: 1,
        },
    );
    let snapshot = shared.snapshot();
    assert!(
        snapshot.agents("build-1").is_empty(),
        "a machine whose link is down claims no agent"
    );
    assert_eq!(snapshot.done + snapshot.needs_you, 0);

    shared.set_link("build-1", WatchLink::Connecting);
    shared.answered("build-1");
    assert_eq!(
        shared.snapshot().machine("build-1").map(|state| state.link),
        Some(WatchLink::Watching),
        "a heartbeat of a link that has not reported reads as watching"
    );
    shared.set_link("build-1", WatchLink::NoTmux);
    shared.answered("build-1");
    assert_eq!(
        shared.snapshot().machine("build-1").map(|state| state.link),
        Some(WatchLink::NoTmux),
        "a heartbeat does not overwrite the note of the machine"
    );
}

#[test]
fn a_machine_that_leaves_and_the_limit_of_the_watch_are_links_of_the_snapshot() {
    let shared = Shared::new();
    shared.set_link("build-1", WatchLink::Watching);
    shared.set_link("build-2", WatchLink::OverTheLimit);
    shared.set_link("build-3", WatchLink::Off);
    let snapshot = shared.snapshot();
    assert_eq!(snapshot.machines.len(), 3);
    assert_eq!(
        snapshot.machine("build-2").map(|state| state.link),
        Some(WatchLink::OverTheLimit)
    );
    assert_eq!(
        snapshot.machine("build-3").map(|state| state.link),
        Some(WatchLink::Off)
    );
    assert!(!WatchLink::OverTheLimit.watching());
    assert!(WatchLink::NoTmux.watching());
    assert!(WatchLink::NoServer.watching());
    assert!(
        !WatchLink::Lost {
            since_millis: 0,
            attempt: 1
        }
        .watching()
    );
    shared.remove("build-1");
    assert!(shared.snapshot().machine("build-1").is_none(), "a stopped watch leaves");
}

#[test]
fn a_refusal_names_its_reason() {
    let refused = WatchRefusal::of_ssh("Warning: Permanently added 'x' to the list.\nHost key verification failed.\n");
    assert_eq!(refused, WatchRefusal::HostKey);
    assert_eq!(
        WatchRefusal::of_ssh("person@machine: Permission denied (publickey)."),
        WatchRefusal::Key
    );
    assert_eq!(
        WatchRefusal::of_ssh("Load key \"/home/person/.ssh/id\": bad permissions"),
        WatchRefusal::Key
    );
    assert_eq!(
        WatchRefusal::of_ssh("ssh: Could not resolve hostname"),
        WatchRefusal::Other
    );
    assert_eq!(WatchRefusal::of_ssh(""), WatchRefusal::Other);
}

#[test]
fn a_preset_of_the_settings_names_an_agent_of_a_machine() {
    let shared = Shared::new();
    let preset = crate::config::PresetConfig {
        name: "Shop".to_owned(),
        alias: None,
        kind: PanelKind::Gemini,
        command: Some("shop-agent".to_owned()),
        args: Vec::new(),
        resume: crate::panel::PanelResume::Fresh,
        ssh_connection: None,
    };
    shared.apply_report("build-1", &report(&[("%0", "shop-agent", "shop")]));
    assert!(
        shared.snapshot().agents("build-1").is_empty(),
        "the program of a preset is no agent before the settings name it"
    );
    shared.set_programs(AgentPrograms::builtin().with_presets(&[preset]));
    shared.apply_report("build-1", &report(&[("%0", "shop-agent", "shop")]));
    let snapshot: std::sync::Arc<MachineWatchSnapshot> = shared.snapshot();
    assert_eq!(snapshot.agents("build-1").len(), 1);
    assert_eq!(snapshot.agents("build-1")[0].kind, PanelKind::Gemini);
}
