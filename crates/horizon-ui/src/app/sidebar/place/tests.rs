//! The entries of the Place menu: where a workspace lives and where it can live.

use horizon_core::{Machine, MachineRegistry, Machines, WorkspacePlace};

use super::{GONE, PlaceEntry, THIS_COMPUTER, entries};

fn record(name: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: "203.0.113.10".to_owned(),
        ..Machine::default()
    }
}

fn machines(names: &[&str]) -> Machines {
    Machines::with_records(
        MachineRegistry::new(std::path::PathBuf::from("/home/peters/.horizon/machines")),
        names.iter().map(|name| record(name)).collect(),
    )
}

fn labels(entries: &[PlaceEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.label.as_str()).collect()
}

#[test]
fn this_computer_is_the_first_entry_and_it_is_the_current_one() {
    let entries = entries(&WorkspacePlace::Local, &machines(&["build-1", "build-2"]));
    assert_eq!(labels(&entries), [THIS_COMPUTER, "build-1", "build-2"]);
    assert!(entries[0].current, "a workspace here says so");
    assert!(!entries[1].current);
    assert!(entries.iter().all(|entry| !entry.gone));
}

#[test]
fn one_entry_for_each_machine_marks_the_one_the_workspace_is_on() {
    let entries = entries(
        &WorkspacePlace::Machine("build-2".to_owned()),
        &machines(&["build-1", "build-2"]),
    );
    assert_eq!(labels(&entries), [THIS_COMPUTER, "build-1", "build-2"]);
    assert!(!entries[0].current);
    assert!(!entries[1].current);
    assert!(
        entries[2].current,
        "the machine of the workspace carries the check mark"
    );
    assert_eq!(entries[2].place, WorkspacePlace::Machine("build-2".to_owned()));
    assert_eq!(
        entries.iter().filter(|entry| entry.current).collect::<Vec<_>>().len(),
        1,
        "one place is current"
    );
}

#[test]
fn a_machine_that_was_forgotten_stays_in_the_menu_marked_gone() {
    let entries = entries(&WorkspacePlace::Machine("old-1".to_owned()), &machines(&["build-1"]));
    assert_eq!(labels(&entries), [THIS_COMPUTER, "build-1", "old-1 (gone)"]);
    assert_eq!(entries[2].label, format!("old-1 {GONE}"));
    assert!(entries[2].gone, "the machine is not in the list anymore");
    assert!(
        entries[2].current,
        "the workspace still lives there, so the entry is its check mark"
    );
    assert!(!entries[1].gone);
}

#[test]
fn a_menu_of_no_machine_offers_this_computer_alone() {
    let alone = entries(&WorkspacePlace::Local, &machines(&[]));
    assert_eq!(labels(&alone), [THIS_COMPUTER]);
    assert!(alone[0].current);
    assert!(
        entries(&WorkspacePlace::Machine("old-1".to_owned()), &machines(&[]))
            .iter()
            .any(|entry| entry.gone && entry.current),
        "a place that names no kept machine is still visible"
    );
}
