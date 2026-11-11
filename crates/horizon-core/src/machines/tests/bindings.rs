//! The key binding file of the machine list, and the store seam it loads through.

use std::{
    os::unix::fs::PermissionsExt,
    sync::{Mutex, PoisonError},
};

use serde_json::json;

use super::super::*;
use super::code;

/// The key binding file of a test home, and the registry beside it.
fn home(temp: &tempfile::TempDir) -> (MachineRegistry, MachineBindings) {
    let directory = temp.path().join("machines");
    (
        MachineRegistry::new(directory.clone()),
        MachineBindings::in_directory(&directory),
    )
}

/// A registry document as an older Horizon wrote it, with the key name inside
/// each record, written where the registry reads it.
fn write_older_registry(registry: &MachineRegistry, machines: &serde_json::Value) {
    std::fs::create_dir_all(registry.root()).expect("the machine directory");
    std::fs::write(registry.path(), json!({ "machines": machines }).to_string()).expect("the registry");
}

fn record(name: &str, host: &str) -> Machine {
    Machine {
        name: name.to_owned(),
        host: host.to_owned(),
        user: Some("root".to_owned()),
        added_at_millis: 1,
        ..Machine::default()
    }
}

#[test]
fn the_key_of_this_computer_is_a_binding_beside_the_list_and_not_part_of_a_record() {
    let temp = tempfile::tempdir().expect("home");
    let (registry, bindings) = home(&temp);
    let machines = Machines::load(registry.clone()).expect("an empty list");
    let (machines, created) = machines
        .with_record(record("build-1", "203.0.113.10"), Some("build-1"))
        .expect("add");
    assert!(created);
    machines.save().expect("the list is written");

    let listed = std::fs::read_to_string(registry.path()).expect("the registry file");
    assert!(
        !listed.contains("\"key\""),
        "a record that another computer may see holds no key name: {listed}"
    );
    let bound: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(bindings.path()).expect("the binding file"))
            .expect("the bindings are JSON");
    assert_eq!(
        bound,
        json!({ "build-1": "build-1" }),
        "the binding file keeps the key name"
    );

    let loaded = Machines::load(registry).expect("the written list");
    assert_eq!(
        loaded.key("build-1"),
        Some("build-1"),
        "a restart reads the binding back"
    );
    assert_eq!(
        loaded.key("build-2"),
        None,
        "a machine without a binding connects with the person's own keys"
    );
}

#[test]
fn a_registry_of_an_older_horizon_moves_its_key_names_on_the_first_load() {
    let temp = tempfile::tempdir().expect("home");
    let (registry, bindings) = home(&temp);
    write_older_registry(
        &registry,
        &json!([
            { "name": "build-1", "host": "203.0.113.10", "key": "build-1", "added_at_millis": 1 },
            { "name": "build-2", "host": "203.0.113.11", "added_at_millis": 2 }
        ]),
    );

    let machines = Machines::load(registry.clone()).expect("the older list");
    assert_eq!(machines.records().len(), 2, "every record of the older list stays");
    assert_eq!(
        machines.key("build-1"),
        Some("build-1"),
        "the key name of the record moved to the binding of this computer"
    );
    assert_eq!(machines.key("build-2"), None);

    let rewritten = std::fs::read_to_string(registry.path()).expect("the rewritten registry");
    assert!(
        !rewritten.contains("\"key\""),
        "the registry is rewritten once without the field: {rewritten}"
    );
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(bindings.path()).expect("the binding file")).expect("JSON");
    assert_eq!(written, json!({ "build-1": "build-1" }));

    let again = Machines::load(registry).expect("the rewritten list");
    assert_eq!(again.records(), machines.records(), "the migration is stable");
    assert_eq!(again.key("build-1"), Some("build-1"));
}

#[test]
fn a_binding_that_is_already_here_wins_over_a_key_name_of_an_older_record() {
    let temp = tempfile::tempdir().expect("home");
    let (registry, bindings) = home(&temp);
    write_older_registry(
        &registry,
        &json!([{ "name": "build-1", "host": "203.0.113.10", "key": "from-the-record" }]),
    );
    std::fs::create_dir_all(registry.root()).expect("the machine directory");
    std::fs::write(bindings.path(), json!({ "build-1": "from-this-computer" }).to_string()).expect("the bindings");

    let machines = Machines::load(registry).expect("the older list");
    assert_eq!(
        machines.key("build-1"),
        Some("from-this-computer"),
        "the key of this computer is the one that this home made"
    );
}

#[test]
fn a_set_without_a_key_forgets_the_binding_and_a_forget_keeps_it() {
    let temp = tempfile::tempdir().expect("home");
    let (registry, _) = home(&temp);
    let machines = Machines::load(registry).expect("an empty list");
    let (machines, _) = machines
        .with_record(record("build-1", "203.0.113.10"), Some("build-1"))
        .expect("add");
    let (replaced, created) = machines
        .with_record(record("build-1", "203.0.113.11"), None)
        .expect("replace");
    assert!(!created);
    assert_eq!(replaced.key("build-1"), None, "a set without a key forgets the old one");

    let (with_key, _) = machines
        .with_record(record("build-2", "203.0.113.12"), Some("build-2"))
        .expect("add");
    let forgotten = with_key.without_record("build-2");
    assert_eq!(forgotten.records().len(), 1);
    assert_eq!(
        forgotten.key("build-2"),
        Some("build-2"),
        "the key of a forgotten machine stays, as its key pair stays"
    );
}

#[test]
fn the_binding_file_is_private_and_leaves_no_temporary_file() {
    let temp = tempfile::tempdir().expect("home");
    let (_, bindings) = home(&temp);
    let mut bindings = bindings;
    assert!(
        bindings.set("build-1", Some("build-1")),
        "a new binding changes the file"
    );
    bindings.save().expect("the binding file");

    assert_eq!(
        std::fs::metadata(bindings.path())
            .expect("the file")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "the binding names the keys of the person"
    );
    assert_eq!(
        std::fs::metadata(bindings.path().parent().expect("the directory"))
            .expect("the directory")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "the directory is private"
    );
    assert!(!bindings.path().with_extension("json.tmp").exists());
    assert!(
        !bindings.set("build-1", Some("build-1")),
        "the same binding changes nothing"
    );
    assert!(bindings.set("build-1", None), "forgetting a binding changes the file");
    assert_eq!(bindings.names().len(), 0);
}

#[test]
fn a_damaged_binding_file_answers_unavailable_instead_of_a_list_without_keys() {
    let temp = tempfile::tempdir().expect("home");
    let (registry, bindings) = home(&temp);
    write_older_registry(&registry, &json!([{ "name": "build-1", "host": "203.0.113.10" }]));
    std::fs::create_dir_all(registry.root()).expect("the machine directory");
    std::fs::write(bindings.path(), b"{not json").expect("a damaged binding file");

    let refused = Machines::load(registry).expect_err("a damaged binding file");
    assert_eq!(code(&refused), MACHINE_UNAVAILABLE);
    assert!(refused.to_string().contains("keys.json"), "{refused}");
}

/// A store that keeps the records in memory, to prove that the list loads
/// through the trait and not through the file alone.
#[derive(Debug, Default)]
struct MemoryStore {
    records: Mutex<Vec<Machine>>,
    saves: Mutex<usize>,
}

impl MachineStore for MemoryStore {
    fn load(&self) -> Result<LoadedMachines, MachineError> {
        Ok(LoadedMachines {
            records: self.records.lock().unwrap_or_else(PoisonError::into_inner).clone(),
            keys: Vec::new(),
        })
    }

    fn save(&self, records: &[Machine]) -> Result<(), MachineError> {
        *self.records.lock().unwrap_or_else(PoisonError::into_inner) = records.to_vec();
        *self.saves.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Ok(())
    }
}

#[test]
fn the_machine_store_is_a_trait_that_another_implementation_can_hold() {
    let store: Box<dyn MachineStore> = Box::new(MemoryStore::default());
    store
        .save(&[record("build-1", "203.0.113.10")])
        .expect("the records are stored");
    let loaded = store.load().expect("the records are read back");
    assert_eq!(loaded.records, vec![record("build-1", "203.0.113.10")]);
    assert!(loaded.keys.is_empty(), "a store of records carries no key binding");
}

/// A store that answers the whole set of key bindings, as the store of a link to a
/// server does.
#[derive(Debug, Default)]
struct ServerStore {
    records: Mutex<Vec<Machine>>,
    keys: Mutex<Vec<(String, String)>>,
}

impl MachineStore for ServerStore {
    fn load(&self) -> Result<LoadedMachines, MachineError> {
        Ok(LoadedMachines {
            records: self.records.lock().unwrap_or_else(PoisonError::into_inner).clone(),
            keys: self.keys.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        })
    }

    fn save(&self, records: &[Machine]) -> Result<(), MachineError> {
        *self.records.lock().unwrap_or_else(PoisonError::into_inner) = records.to_vec();
        Ok(())
    }

    fn holds_bindings(&self) -> bool {
        true
    }
}

#[test]
fn a_store_of_a_server_decides_the_bindings_and_a_file_store_fills_a_gap() {
    let temp = tempfile::tempdir().expect("a directory");
    let (registry, mut bindings) = home(&temp);
    bindings.set("build-1", Some("one"));
    bindings.set("build-2", Some("two"));
    bindings.save().expect("the bindings before the server");

    // The file store holds the bindings itself: a load through it keeps them.
    let file = Machines::from_store(&registry, std::sync::Arc::new(registry.clone())).expect("the file store");
    assert_eq!(file.key("build-1"), Some("one"));
    assert_eq!(file.key("build-2"), Some("two"));

    // A store of a server answers the whole set: the binding that the server
    // removed goes, and the one that it added comes.
    let server = std::sync::Arc::new(ServerStore::default());
    *server.keys.lock().expect("the keys") = vec![("build-1".to_owned(), "theirs".to_owned())];
    let machines = Machines::from_store(&registry, server).expect("the store of the server");
    assert_eq!(machines.key("build-1"), Some("theirs"), "the server decides");
    assert_eq!(machines.key("build-2"), None, "a binding of the server wins");
    let file = registry.bindings().load().expect("the binding file");
    assert_eq!(file.key("build-1"), Some("theirs"));
    assert_eq!(file.key("build-2"), None, "the file holds the set of the server");
}
