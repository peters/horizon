//! The machine list, judged on a temporary Horizon home.

use std::os::unix::fs::PermissionsExt;

use horizon_browser_control::manifest::block_terminal::{self, TerminalRoute};

use crate::ssh::MachineKeyRef;

use super::*;

fn registry(temp: &tempfile::TempDir) -> MachineRegistry {
    MachineRegistry::new(temp.path().join("machines"))
}

fn route(host: &str) -> TerminalRoute {
    TerminalRoute {
        host: host.to_owned(),
        user: Some("root".to_owned()),
        port: None,
    }
}

fn checked(name: &str, host: &str, session: Option<&str>) -> CheckedMachine {
    check(
        name,
        &route(host),
        session,
        Some("Hetzner, hel1"),
        Some("example-org/shop"),
    )
    .expect("the machine is accepted")
}

fn code(error: &MachineError) -> &'static str {
    error.code()
}

#[test]
fn a_record_keeps_what_the_agent_sent_and_the_list_is_in_name_order() {
    let temp = tempfile::tempdir().expect("home");
    let machines = Machines::load(registry(&temp)).expect("an empty list");

    let (machines, created) = machines
        .with_record(checked("build-2", " 203.0.113.10 ", None).record(1), None)
        .expect("add");
    assert!(created, "a new name is created");
    let (machines, created) = machines
        .with_record(checked("build-1", "203.0.113.11", Some("work")).record(2), None)
        .expect("add");
    assert!(created);

    assert_eq!(
        machines
            .records()
            .iter()
            .map(|machine| machine.name.as_str())
            .collect::<Vec<_>>(),
        ["build-1", "build-2"],
        "the list is in name order whatever the order of the adds"
    );
    let first = machines.find("build-1").expect("the record");
    assert_eq!(first.host, "203.0.113.11", "the route is normalised");
    assert_eq!(first.user.as_deref(), Some("root"));
    assert_eq!(
        machines.key("build-1"),
        None,
        "a record carries no key name: the key of this computer is a local binding"
    );
    assert_eq!(first.session.as_deref(), Some("work"));
    assert_eq!(first.provider.as_deref(), Some("Hetzner, hel1"));
    assert_eq!(first.added_at_millis, 2);
    assert_eq!(machines.find("build-9"), None);
}

#[test]
fn a_second_set_of_one_name_replaces_it_and_keeps_the_day_it_was_added() {
    let temp = tempfile::tempdir().expect("home");
    let machines = Machines::load(registry(&temp)).expect("an empty list");
    let (machines, _) = machines
        .with_record(checked("build-1", "203.0.113.10", None).record(1), None)
        .expect("add");

    let (machines, created) = machines
        .with_record(checked("build-1", "203.0.113.11", Some("work")).record(9), None)
        .expect("replace");
    assert!(!created, "a name that is there is replaced, not created");
    assert_eq!(machines.records().len(), 1);
    let record = machines.find("build-1").expect("the record");
    assert_eq!(record.host, "203.0.113.11");
    assert_eq!(record.session.as_deref(), Some("work"));
    assert_eq!(record.added_at_millis, 1, "the day of the first record stays");
}

#[test]
fn a_delete_forgets_one_record_and_leaves_the_rest() {
    let temp = tempfile::tempdir().expect("home");
    let machines = Machines::load(registry(&temp)).expect("an empty list");
    let (machines, _) = machines
        .with_record(checked("build-1", "203.0.113.10", None).record(1), None)
        .expect("add");
    let (machines, _) = machines
        .with_record(checked("build-2", "203.0.113.11", None).record(2), None)
        .expect("add");

    let after = machines.without_record("build-1");
    assert_eq!(after.records().len(), 1);
    assert_eq!(
        after.find("build-2").map(|machine| machine.name.as_str()),
        Some("build-2")
    );
    assert_eq!(machines.records().len(), 2, "the old list is untouched");
    assert_eq!(after.without_record("build-2").records().len(), 0);
    assert_eq!(
        after.without_record("build-9").records().len(),
        1,
        "an unknown name changes nothing"
    );
}

#[test]
fn a_save_and_a_load_read_the_same_list_back_from_a_temporary_home() {
    let temp = tempfile::tempdir().expect("home");
    let registry = registry(&temp);
    let machines = Machines::load(registry.clone()).expect("an empty list");
    let (machines, _) = machines
        .with_record(checked("build-1", "203.0.113.10", Some("work")).record(1), None)
        .expect("add");
    let (machines, _) = machines
        .with_record(checked("build-2", "203.0.113.11", None).record(2), None)
        .expect("add");
    machines.save().expect("the list is written");

    let path = registry.path();
    assert!(path.is_file(), "the registry file exists");
    assert_eq!(
        std::fs::metadata(&path).expect("the file").permissions().mode() & 0o777,
        0o600,
        "the list names hosts, so it stays private"
    );
    assert_eq!(
        std::fs::metadata(path.parent().expect("the directory"))
            .expect("the directory")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "the directory is private"
    );
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the temporary file is renamed, not left behind"
    );

    let loaded = Machines::load(registry).expect("the written list");
    assert_eq!(loaded.records(), machines.records(), "the same records come back");
}

#[test]
fn a_damaged_registry_file_answers_unavailable_instead_of_an_empty_list() {
    let temp = tempfile::tempdir().expect("home");
    let registry = registry(&temp);
    std::fs::create_dir_all(registry.path().parent().expect("the directory")).expect("the directory");
    std::fs::write(registry.path(), b"{not json").expect("a damaged file");

    let refused = Machines::load(registry).expect_err("a damaged list");
    assert_eq!(code(&refused), MACHINE_UNAVAILABLE);
    assert!(refused.to_string().contains("registry.json"), "{refused}");
}

#[test]
fn the_limit_counts_new_names_only_and_is_reported_with_its_own_code() {
    let temp = tempfile::tempdir().expect("home");
    let mut machines = Machines::load(registry(&temp)).expect("an empty list");
    for index in 0..MAX_MACHINES {
        let name = format!("build-{index}");
        let (next, _) = machines
            .with_record(checked(&name, "203.0.113.10", None).record(1), None)
            .expect("a machine below the limit");
        machines = next;
    }
    let refused = machines
        .with_record(checked("build-last", "203.0.113.10", None).record(1), None)
        .expect_err("the limit");
    assert_eq!(code(&refused), TOO_MANY_MACHINES);
    assert!(refused.to_string().contains("Forget one"), "{refused}");
    let (replaced, created) = machines
        .with_record(checked("build-3", "203.0.113.11", None).record(2), None)
        .expect("a name that is there is not refused");
    assert!(!created);
    assert_eq!(replaced.records().len(), MAX_MACHINES);
}

#[test]
fn every_refusal_names_its_field_and_its_code() {
    let refused = |name: &str, route: TerminalRoute, session: Option<&str>| {
        let error = check(name, &route, session, None, None).expect_err("the record is refused");
        (code(&error).to_owned(), error.to_string())
    };

    for name in ["", "Build", "build_1", "../build", "-build", &"a".repeat(41)] {
        let (error_code, message) = refused(name, route("203.0.113.10"), None);
        assert_eq!(error_code, INVALID_MACHINE_NAME, "{name:?}");
        assert!(message.contains("name"), "{name:?} refuses without the rule: {message}");
    }
    for host in ["", "-oProxyCommand=id", "lab 1", "lab\t1"] {
        let (error_code, message) = refused("build-1", route(host), None);
        assert_eq!(error_code, block_terminal::INVALID_SSH_ROUTE, "{host:?}");
        assert!(
            message.contains("ssh.host"),
            "{host:?} refuses without the field: {message}"
        );
    }
    let mut port = route("203.0.113.10");
    port.port = Some(0);
    let (error_code, message) = refused("build-1", port, None);
    assert_eq!(error_code, block_terminal::INVALID_SSH_ROUTE);
    assert!(message.contains("ssh.port"), "{message}");
    for key in ["", "Build", "build 1", "build-1.pub"] {
        let error = machine_key_name(key).expect_err("the key name is refused");
        assert_eq!(
            code(&error),
            horizon_browser_control::manifest::block_keys::INVALID_KEY_NAME,
            "{key:?}"
        );
        assert!(error.to_string().contains("name"), "{error}");
    }
    assert_eq!(
        machine_key_name("build-1").expect("a plain key name"),
        "build-1",
        "a key name is checked on its own, beside the record"
    );
    for session in ["", "-rf", "build session", "build-1:x", "$(id)", &"a".repeat(41)] {
        let (error_code, message) = refused("build-1", route("203.0.113.10"), Some(session));
        assert_eq!(error_code, INVALID_SESSION_NAME, "{session:?}");
        assert!(message.contains("session"), "{message}");
    }
    let longest = "a".repeat(MAX_SESSION_CHARS);
    let accepted =
        check("build-1", &route("203.0.113.10"), Some(&longest), None, None).expect("a session at the bound");
    assert_eq!(accepted.session.as_deref(), Some(longest.as_str()));
    let named = check("build-1", &route("203.0.113.10"), Some("Build_1-x"), None, None).expect("a plain session");
    assert_eq!(named.session.as_deref(), Some("Build_1-x"));
}

#[test]
fn a_provider_line_is_one_short_line_that_the_record_cuts_to_the_bound() {
    let record = check(
        "build-1",
        &route("203.0.113.10"),
        None,
        Some("  Hetzner,\n hel1   running  "),
        None,
    )
    .expect("a record");
    assert_eq!(record.provider.as_deref(), Some("Hetzner, hel1 running"));
    let long = "a".repeat(MAX_PROVIDER_CHARS + 20);
    let record = check("build-1", &route("203.0.113.10"), None, Some(&long), None).expect("a record");
    assert_eq!(
        record.provider.as_deref().map(str::len),
        Some(MAX_PROVIDER_CHARS),
        "a long provider line is cut"
    );
    assert_eq!(
        check("build-1", &route("203.0.113.10"), None, None, None)
            .expect("a record")
            .provider,
        None,
        "no provider line is a record without one"
    );
}

#[test]
fn a_record_of_an_older_version_reads_without_a_repository_and_one_with_it_round_trips() {
    let temp = tempfile::tempdir().expect("home");
    // A record of an older Horizon: the file has no `repository` field, and the
    // struct fills it with nothing.
    let older = serde_json::json!({
        "machines": [{
            "name": "build-1",
            "host": "203.0.113.10",
            "user": "root",
            "provider": "Hetzner, hel1",
            "added_at_millis": 7
        }]
    });
    std::fs::create_dir_all(temp.path().join("machines")).expect("the machine folder");
    std::fs::write(
        temp.path().join("machines/registry.json"),
        serde_json::to_vec(&older).expect("encode"),
    )
    .expect("write");
    let loaded = Machines::load(registry(&temp)).expect("an older record reads");
    assert_eq!(
        loaded.records()[0].repository,
        None,
        "a record without the field reads with no repository"
    );

    let (loaded, _) = loaded
        .with_record(checked("build-1", "203.0.113.10", None).record(7), None)
        .expect("the record is kept");
    loaded.save().expect("the list is written");
    let text = std::fs::read_to_string(temp.path().join("machines/registry.json")).expect("the file");
    assert!(
        text.contains("\"repository\": \"example-org/shop\""),
        "the record holds the repository: {text}"
    );
    let again = Machines::load(registry(&temp)).expect("the written record reads");
    assert_eq!(
        again.records()[0].repository.as_deref(),
        Some("example-org/shop"),
        "a record with the field reads it again"
    );

    // An older Horizon refuses a field that it does not know, so every computer
    // that follows one server must know the field before a machine gets one.
    let with_unknown = serde_json::json!({
        "machines": [{
            "name": "build-1",
            "host": "203.0.113.10",
            "added_at_millis": 7,
            "another_field": "x"
        }]
    });
    let refused = serde_json::from_value::<Machine>(with_unknown["machines"][0].clone());
    assert!(
        refused.is_err(),
        "a record of a newer version is refused by name, not read in part"
    );
}

/// The key of a panel, as the store resolves it.
fn key(name: &str) -> MachineKeyRef {
    MachineKeyRef {
        directory: "/home/peters/.horizon/machine-keys".to_owned(),
        name: name.to_owned(),
    }
}

#[test]
fn the_argv_of_a_machine_without_a_session_is_the_plain_login() {
    let machine = checked("build-1", "203.0.113.10", None).record(1);
    let args = machine.ssh_args(&key("build-1")).expect("the argv");
    let directory = "/home/peters/.horizon/machine-keys";
    assert_eq!(
        args,
        vec![
            "-i".to_string(),
            format!("{directory}/build-1"),
            "-o".to_string(),
            "IdentitiesOnly=yes".to_string(),
            "-o".to_string(),
            "IdentityAgent=none".to_string(),
            "-o".to_string(),
            format!("UserKnownHostsFile={directory}/build-1.known_hosts"),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
            "-o".to_string(),
            "ServerAliveInterval=5".to_string(),
            "-o".to_string(),
            "ServerAliveCountMax=3".to_string(),
            "root@203.0.113.10".to_string(),
        ],
        "a machine without a session opens a login shell, with the bounds of a machine"
    );
}

#[test]
fn the_argv_of_a_machine_with_a_session_attaches_that_tmux_session() {
    let machine = checked("build-1", "203.0.113.10", Some("work")).record(1);
    let args = machine.ssh_args(&key("build-1")).expect("the argv");
    assert_eq!(args[args.len() - 3], "-t", "a remote command needs a terminal");
    assert_eq!(args[args.len() - 2], "root@203.0.113.10");
    assert_eq!(
        args[args.len() - 1],
        "tmux -u new-session -A -s work \\; set-option status off",
        "a click on a machine hides the tmux status line"
    );
    assert_eq!(machine.ssh_args(&key("build-1")).expect("argv"), args);
}

#[test]
fn a_click_on_a_machine_and_a_terminal_of_a_place_build_one_command() {
    // The two paths that open a terminal of a machine: the click on the machine
    // row, and a new terminal of a workspace that is placed on the machine.
    let registry = MachineRegistry::new(std::env::temp_dir().join("hz-machine-command-test"));
    let machines = Machines::with_records(
        registry,
        vec![checked("build-1", "203.0.113.10", Some("work")).record(1)],
    )
    .with_key("build-1", "build-1");
    let keys = crate::MachineKeys::new(std::env::temp_dir().join("hz-machine-command-test-keys"));
    let mut options = crate::PanelOptions {
        kind: crate::PanelKind::Shell,
        ..crate::PanelOptions::default()
    };
    crate::place_command(
        &mut options,
        &crate::WorkspacePlace::Machine("build-1".to_owned()),
        &machines,
        &keys,
        &crate::place::PlaceWorkspace {
            local_id: "c8dfe3af-0e07-4889-99ed-403b4a5a4c8a",
            sessions: &[],
        },
    )
    .expect("the place of the workspace");
    let placed = options
        .ssh_connection
        .expect("the connection of the place")
        .to_command_args();
    let clicked = machines
        .find("build-1")
        .expect("the machine")
        .ssh_args(&keys.binding("build-1"))
        .expect("the argv of the click");
    for argv in [&placed, &clicked] {
        for expected in ["ConnectTimeout=10", "ServerAliveInterval=5", "ServerAliveCountMax=3"] {
            assert!(
                argv.iter().any(|argument| argument == expected),
                "a machine terminal bounds its attempt with {expected}: {argv:?}"
            );
        }
    }
    let placed = placed.last().expect("the command of the place").clone();
    let clicked = clicked.last().expect("the command of the click").clone();
    assert_eq!(
        clicked,
        crate::machines::session_command("work"),
        "the click runs the one builder"
    );
    let suffix = "\\; set-option status off";
    assert!(
        placed.starts_with("tmux -u new-session -A -s hz-c8dfe3af-1 ")
            && clicked.starts_with("tmux -u new-session -A -s work "),
        "both attach a session of their own: {placed} and {clicked}"
    );
    assert!(
        placed.ends_with(suffix) && clicked.ends_with(suffix),
        "a placed terminal and a click hide the status line with one command: {placed} and {clicked}"
    );
}

#[test]
fn a_record_that_no_longer_passes_the_rule_builds_no_terminal() {
    let mut machine = checked("build-1", "203.0.113.10", Some("work")).record(1);
    machine.host = "-oProxyCommand=touch /var/tmp/hz-c7".to_owned();
    let refused = machine.ssh_args(&key("build-1")).expect_err("an edited route");
    assert_eq!(code(&refused), block_terminal::INVALID_SSH_ROUTE);

    let mut machine = checked("build-1", "203.0.113.10", Some("work")).record(1);
    machine.session = Some("work; id".to_owned());
    let refused = machine.checked_terminal().expect_err("an edited session");
    assert_eq!(code(&refused), INVALID_SESSION_NAME);
    assert!(
        machine.checked_terminal().is_err(),
        "Horizon builds the tmux command itself, so a saved one cannot reach it"
    );
}

#[test]
fn a_machine_name_is_the_key_name_rule() {
    for name in ["build-1", "a", "0", "rented-machine-17"] {
        assert_eq!(machine_name(name).expect("a plain name"), name);
        assert_eq!(
            check(name, &route("203.0.113.10"), None, None, None)
                .expect("a record")
                .name,
            name,
            "one name makes the record"
        );
    }
    for name in ["build-1.pub", "build-1.known_hosts"] {
        assert_eq!(
            code(&machine_name(name).expect_err("a file of the directory")),
            INVALID_MACHINE_NAME
        );
    }
}

#[test]
fn the_codes_are_the_ones_the_tool_reports() {
    assert_eq!(INVALID_MACHINE_NAME, "invalid_machine_name");
    assert_eq!(INVALID_SESSION_NAME, "invalid_session_name");
    assert_eq!(MACHINE_UNAVAILABLE, "machine_unavailable");
    assert_eq!(TOO_MANY_MACHINES, "too_many_machines");
    assert_eq!(MAX_MACHINES, 64);
    assert_eq!(MAX_PROVIDER_CHARS, 60);
    assert_eq!(MAX_SESSION_CHARS, 40);
}

mod bindings;
