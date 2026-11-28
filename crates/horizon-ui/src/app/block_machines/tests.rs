//! The machine list that agents write, judged on a test board.

use std::fs;
use std::path::{Path, PathBuf};

use egui::Context;
use horizon_browser_control::manifest::block::{self, Outcome};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::browser::manifest;
use horizon_core::{Machine, MachineKeys, MachineRegistry, Machines, PanelKind, RuntimeState, StartupDecision};

use super::*;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

type TestApp = (tempfile::TempDir, Context, HorizonApp);

fn app() -> TestApp {
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
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    // The startup read of the machine list answers before a test writes one.
    settle_machine_load(&mut app);
    (temp, ctx, app)
}

/// The queue directory of a test, so no answer reaches the runtime root of the
/// person running the test.
fn queue(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("block-panel-requests");
    fs::create_dir_all(&root).expect("the queue directory");
    root
}

/// Writes a key pair of `name` where the store looks for it, so a test of a
/// record needs no `ssh-keygen`.
fn plant_key(app: &HorizonApp, name: &str) {
    let directory = MachineKeys::from_home(app.session_store.home()).key_directory();
    fs::create_dir_all(&directory).expect("the key directory");
    fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the private key");
    fs::write(
        directory.join(format!("{name}.pub")),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI horizon-machine-build-1\n",
    )
    .expect("the public key");
}

fn request(app: &HorizonApp, operation: block::Operation) -> block::Request {
    block::Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[0].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 30_000,
        operation,
    }
}

fn set(name: &str, host: &str, key: Option<&str>, session: Option<&str>) -> block::Operation {
    block::Operation::MachineSet {
        name: name.into(),
        ssh: TerminalRoute {
            host: host.into(),
            user: Some("root".into()),
            port: None,
        },
        key: key.map(str::to_string),
        session: session.map(str::to_string),
        provider: Some("Hetzner, hel1".into()),
        repository: None,
        folder: None,
        size: None,
        place: None,
        price_per_hour: None,
        cards: false,
    }
}

/// The same record as [`set`], ready for `repository`.
fn set_ready_for(name: &str, repository: Option<&str>) -> block::Operation {
    let block::Operation::MachineSet {
        name: record_name,
        ssh,
        key,
        session,
        provider,
        ..
    } = set(name, "203.0.113.10", Some("build-1"), None)
    else {
        unreachable!("an operation of one machine")
    };
    block::Operation::MachineSet {
        name: record_name,
        ssh,
        key,
        session,
        provider,
        repository: repository.map(str::to_string),
        folder: None,
        size: None,
        place: None,
        price_per_hour: None,
        cards: false,
    }
}

/// The same record as [`set_ready_for`], in the folder that the machine holds the work.
fn set_in_folder(name: &str, folder: Option<&str>) -> block::Operation {
    let block::Operation::MachineSet {
        name,
        ssh,
        key,
        session,
        provider,
        repository,
        ..
    } = set_ready_for(name, Some("example-org/shop"))
    else {
        unreachable!("an operation of one machine")
    };
    block::Operation::MachineSet {
        name,
        ssh,
        key,
        session,
        provider,
        repository,
        folder: folder.map(str::to_string),
        size: None,
        place: None,
        price_per_hour: None,
        cards: false,
    }
}

/// The record of [`set_in_folder`] with the three facts of a rented machine.
fn set_with_facts(
    name: &str,
    size: Option<&str>,
    place: Option<&str>,
    price_per_hour: Option<&str>,
) -> block::Operation {
    let block::Operation::MachineSet {
        name,
        ssh,
        key,
        session,
        provider,
        repository,
        folder,
        ..
    } = set_in_folder(name, None)
    else {
        unreachable!("an operation of one machine")
    };
    block::Operation::MachineSet {
        name,
        ssh,
        key,
        session,
        provider,
        repository,
        folder,
        size: size.map(str::to_string),
        place: place.map(str::to_string),
        price_per_hour: price_per_hour.map(str::to_string),
        cards: false,
    }
}

/// Applies one request that the coordination worker answers, and returns its
/// answer as the asking agent reads it.
fn answered(app: &mut HorizonApp, ctx: &Context, root: &Path, operation: block::Operation) -> Outcome {
    let request = request(app, operation);
    assert!(
        app.apply_block_request_in(&request, root, ctx).is_none(),
        "a machine operation answers from the coordination worker"
    );
    app.settle_browser_host_io();
    block::take_result_at(root, &request)
        .expect("the answer is readable")
        .expect("the worker answered")
}

fn machines(app: &mut HorizonApp, ctx: &Context, root: &Path) -> Vec<block::MachineSummary> {
    match answered(app, ctx, root, block::Operation::Machines) {
        Outcome::Machines { machines, .. } => machines,
        other => panic!("expected the machine list: {other:?}"),
    }
}

fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn a_machine_set_writes_the_record_of_this_horizon_home() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");

    let answer = answered(
        &mut app,
        &ctx,
        &root,
        set("build-1", "203.0.113.10", Some("build-1"), Some("work")),
    );
    assert!(
        matches!(&answer, Outcome::MachineSet { name, created } if name == "build-1" && *created),
        "a new name is created: {answer:?}"
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(listed.len(), 1);
    let record = &listed[0];
    assert_eq!(record.name, "build-1");
    assert_eq!(record.host, "203.0.113.10");
    assert_eq!(record.user.as_deref(), Some("root"));
    assert_eq!(record.key.as_deref(), Some("build-1"));
    assert_eq!(record.session.as_deref(), Some("work"));
    assert_eq!(record.provider.as_deref(), Some("Hetzner, hel1"));
    assert!(!record.terminal_open, "no terminal of the machine is open yet");
    assert!(record.added_at_millis > 0, "the record says when it was added");
    assert!(!record.key_missing, "the key of the record is on this computer");

    let path = MachineRegistry::from_home(app.session_store.home()).path();
    assert!(
        path.is_file(),
        "the record is a file of the Horizon home: {}",
        path.display()
    );
    let reloaded = Machines::load(MachineRegistry::from_home(app.session_store.home())).expect("the list");
    assert_eq!(reloaded.records().len(), 1, "a restart of Horizon reads the same list");
    assert_eq!(reloaded.records()[0].name, "build-1");

    let again = answered(&mut app, &ctx, &root, set("build-1", "203.0.113.11", None, None));
    assert!(
        matches!(&again, Outcome::MachineSet { created, .. } if !created),
        "a second set of one name replaces it: {again:?}"
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].host, "203.0.113.11");
    assert_eq!(listed[0].key, None, "a set without a key forgets the old one");
}

#[test]
fn a_machine_set_keeps_the_repository_and_machines_answers_it() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");

    let _ = answered(
        &mut app,
        &ctx,
        &root,
        set_ready_for("build-1", Some(" example-org/shop ")),
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        listed[0].repository.as_deref(),
        Some("example-org/shop"),
        "the record keeps the repository of the agent, trimmed"
    );
    let reloaded = Machines::load(MachineRegistry::from_home(app.session_store.home())).expect("the list");
    assert_eq!(
        reloaded.records()[0].repository.as_deref(),
        Some("example-org/shop"),
        "a restart of Horizon reads the same repository"
    );

    let _ = answered(&mut app, &ctx, &root, set_ready_for("build-1", None));
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        listed[0].repository, None,
        "a set without a repository forgets the one before"
    );
}

#[test]
fn a_machine_set_keeps_the_folder_and_machines_answers_it() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");

    let _ = answered(
        &mut app,
        &ctx,
        &root,
        set_in_folder("build-1", Some("/home/dev/example-repo")),
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        listed[0].folder.as_deref(),
        Some("/home/dev/example-repo"),
        "the answer of machines names the folder of the new terminals"
    );
    let reloaded = Machines::load(MachineRegistry::from_home(app.session_store.home())).expect("the list");
    assert_eq!(
        reloaded.records()[0].folder.as_deref(),
        Some("/home/dev/example-repo"),
        "a restart of Horizon reads the same folder"
    );

    let _ = answered(&mut app, &ctx, &root, set_in_folder("build-1", None));
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(listed[0].folder, None, "a set without a folder forgets the one before");
}

#[test]
fn a_machine_set_keeps_the_size_the_place_and_the_price_and_machines_answers_them() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");

    let answer = answered(
        &mut app,
        &ctx,
        &root,
        set_with_facts("build-1", Some("cpx32"), Some("Helsinki"), Some("0.03 EUR")),
    );
    assert!(
        matches!(&answer, Outcome::MachineSet { created, .. } if *created),
        "the record of the rented machine: {answer:?}"
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        (
            listed[0].size.as_deref(),
            listed[0].place.as_deref(),
            listed[0].price_per_hour.as_deref()
        ),
        (Some("cpx32"), Some("Helsinki"), Some("0.03 EUR")),
        "machines answers the three facts of the rented machine"
    );
    let reloaded = Machines::load(MachineRegistry::from_home(app.session_store.home())).expect("the list");
    assert_eq!(
        reloaded.records()[0].place.as_deref(),
        Some("Helsinki"),
        "a restart of Horizon reads the same place"
    );

    let _ = answered(
        &mut app,
        &ctx,
        &root,
        set_with_facts("build-1", None, None, Some("0.05 EUR")),
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        (
            listed[0].size.as_deref(),
            listed[0].place.as_deref(),
            listed[0].price_per_hour.as_deref()
        ),
        (Some("cpx32"), Some("Helsinki"), Some("0.05 EUR")),
        "a set that names no size and no place keeps the ones of the record"
    );

    let _ = answered(
        &mut app,
        &ctx,
        &root,
        set_with_facts("build-1", Some(""), Some(""), None),
    );
    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        (
            listed[0].size.as_deref(),
            listed[0].place.as_deref(),
            listed[0].price_per_hour.as_deref()
        ),
        (None, None, Some("0.05 EUR")),
        "an empty text clears a fact and a field that the caller leaves out keeps it"
    );
}

#[test]
fn a_fact_of_a_rented_machine_that_breaks_its_rule_is_refused_with_its_field() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");

    for (size, place, price, field) in [
        (Some("p".repeat(41)), None, None, "size"),
        (None, Some("Helsinki\nEspoo".to_string()), None, "place"),
        (None, None, Some("\u{7}".to_string()), "price_per_hour"),
    ] {
        let answer = answered(
            &mut app,
            &ctx,
            &root,
            set_with_facts("build-1", size.as_deref(), place.as_deref(), price.as_deref()),
        );
        assert_eq!(code(&answer), "invalid_machine_fact", "{field}: {answer:?}");
        let words = format!("{answer:?}");
        assert!(words.contains(field), "the refusal names the field {field}: {words}");
        assert!(
            machines(&mut app, &ctx, &root).is_empty(),
            "a refused fact writes no record"
        );
    }
}

#[test]
fn a_machine_whose_key_was_deleted_reports_the_missing_key() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(
        &mut app,
        &ctx,
        &root,
        set("build-1", "203.0.113.10", Some("build-1"), None),
    );
    assert!(!machines(&mut app, &ctx, &root)[0].key_missing);

    // The person deletes the key on this computer, as the key operation does. The
    // record keeps its binding, so the machine says that the key is not here.
    fs::remove_file(
        MachineKeys::from_home(app.session_store.home())
            .key_directory()
            .join("build-1"),
    )
    .expect("the private key is deleted");

    let listed = machines(&mut app, &ctx, &root);
    assert_eq!(
        listed[0].key.as_deref(),
        Some("build-1"),
        "the record keeps its binding"
    );
    assert!(
        listed[0].key_missing,
        "the answer says that the key of the machine is not on this computer"
    );
}

#[test]
fn a_machine_delete_forgets_the_record_and_leaves_its_key() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(
        &mut app,
        &ctx,
        &root,
        set("build-1", "203.0.113.10", Some("build-1"), None),
    );
    answered(&mut app, &ctx, &root, set("build-2", "203.0.113.11", None, None));

    let answer = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineDelete { name: "build-1".into() },
    );
    assert!(matches!(&answer, Outcome::MachineDeleted { name } if name == "build-1"));
    assert_eq!(
        machines(&mut app, &ctx, &root)
            .iter()
            .map(|machine| machine.name.as_str())
            .collect::<Vec<_>>(),
        ["build-2"],
        "only the named record is forgotten"
    );
    assert!(
        MachineKeys::from_home(app.session_store.home())
            .key_directory()
            .join("build-1")
            .is_file(),
        "a delete of a record never deletes the key: delete_machine_key does that"
    );

    let refused = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineDelete { name: "build-1".into() },
    );
    assert_eq!(code(&refused), "machine_unavailable");
}

#[test]
fn a_machine_that_breaks_a_rule_is_refused_before_any_file_work() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);

    for (operation, expected) in [
        (set("Build", "203.0.113.10", None, None), "invalid_machine_name"),
        (set("build-1", "203.0.113.10", Some("../id"), None), "invalid_key_name"),
        (
            set("build-1", "203.0.113.10", None, Some("work; id")),
            "invalid_session_name",
        ),
        (
            set("build-1", "203.0.113.10", None, Some("-rf")),
            "invalid_session_name",
        ),
        (set("build-1", "-oProxyCommand=id", None, None), "invalid_ssh_route"),
        (set("build-1", "203.0.113.10", Some("build-9"), None), "key_unavailable"),
    ] {
        let refused = answered(&mut app, &ctx, &root, operation);
        assert_eq!(code(&refused), expected, "{refused:?}");
    }
    assert!(
        !MachineRegistry::from_home(app.session_store.home()).path().exists(),
        "a refused record writes no list"
    );
    assert!(
        app.machine_list.records().is_empty(),
        "a refused record leaves the list in memory empty too"
    );
}

#[test]
fn a_forget_from_the_sidebar_removes_the_row_and_keeps_the_key() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(
        &mut app,
        &ctx,
        &root,
        set("build-1", "203.0.113.10", Some("build-1"), None),
    );

    app.forget_machine("build-1", &ctx);
    app.settle_browser_host_io();
    assert!(app.machine_list.records().is_empty(), "the row is gone");
    assert!(
        MachineKeys::from_home(app.session_store.home())
            .key_directory()
            .join("build-1")
            .is_file(),
        "a forget removes the record only: delete_machine_key removes the key"
    );
    let reloaded = Machines::load(MachineRegistry::from_home(app.session_store.home())).expect("the list");
    assert!(reloaded.records().is_empty(), "the file no longer holds the record");
}

#[test]
fn a_start_without_a_server_setting_keeps_the_file_store_and_starts_no_link() {
    let (_temp, _ctx, mut app) = app();
    settle_machine_load(&mut app);
    assert!(
        app.server_sync.is_none(),
        "a start that names no server starts no link and no child"
    );
    assert!(
        app.pending_machines().is_empty(),
        "a machine list of one computer has nothing pending"
    );
    let registry = app.machine_registry();
    let record = Machine {
        name: "build-9".to_owned(),
        host: "203.0.113.10".to_owned(),
        added_at_millis: 3,
        ..Machine::default()
    };
    let list = Machines::from_store(&registry, app.machine_store(&registry))
        .expect("the store of this computer")
        .with_record(record, None)
        .expect("a new record")
        .0;
    list.save().expect("the record is written");
    assert!(
        registry.path().is_file(),
        "without a server the machine list is the file of this home"
    );
}

#[test]
fn a_machine_record_carries_no_command_line_of_its_own() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(
        &mut app,
        &ctx,
        &root,
        set("build-1", "203.0.113.10", Some("build-1"), Some("work")),
    );
    let saved = fs::read_to_string(MachineRegistry::from_home(app.session_store.home()).path()).expect("the file");
    assert!(saved.contains("\"session\": \"work\""), "{saved}");
    assert!(
        !saved.contains("tmux new"),
        "the tmux command is Horizon's own text, built when the panel opens: {saved}"
    );
    assert!(!saved.contains("ssh "), "a record is data, not a command line: {saved}");
}

#[test]
fn a_machine_request_of_an_unknown_caller_answers_caller_unavailable() {
    let (_temp, ctx, mut app) = app();
    let mut unknown_caller = request(&app, block::Operation::Machines);
    unknown_caller.actor = "horizon:not-a-panel-here".into();
    assert_eq!(
        code(&app.apply_block_request(&unknown_caller, &ctx)),
        "caller_unavailable"
    );

    let mut other_host = request(&app, set("build-1", "203.0.113.10", None, None));
    other_host.host_instance = "another-host".into();
    assert_eq!(code(&app.apply_block_request(&other_host, &ctx)), "wrong_host");
}

#[test]
fn the_machine_list_of_the_home_belongs_to_every_workspace() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    answered(&mut app, &ctx, &root, set("build-1", "203.0.113.10", None, None));

    // The second workspace's agent panel lists the record that the first wrote.
    let mut second = request(&app, block::Operation::Machines);
    second.actor = format!("horizon:{}", app.board.panels[1].local_id);
    assert!(app.apply_block_request_in(&second, &root, &ctx).is_none());
    app.settle_browser_host_io();
    let answer = block::take_result_at(&root, &second)
        .expect("the answer is readable")
        .expect("the worker answered");
    assert!(
        matches!(&answer, Outcome::Machines { machines, .. } if machines.len() == 1),
        "the machines are the person's, not one workspace's: {answer:?}"
    );
}

/// A first link to a server that never answers keeps the machine and the key
/// binding that this computer held before the press: the sidebar of the person
/// does not empty itself at `Link`, and the key binding stays on its disk.
#[test]
fn a_first_link_to_an_away_server_keeps_the_machine_list_of_this_computer() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(
        &mut app,
        &ctx,
        &root,
        set("build-1", "203.0.113.10", Some("build-1"), None),
    );

    // The press on `Link` of the card: the entry names a server, and the link of
    // it starts with a stand-in `ssh` that no address answers.
    let stand_in = horizon_core::stand_in_program_in(
        temp.path(),
        "ssh",
        "printf 'ssh: connect to host server-vm port 22: Connection refused\\n' >&2\nexit 255",
    )
    .expect("the stand-in ssh");
    app.link_child = Some(stand_in.to_string_lossy().into_owned());
    app.template_config.server = horizon_core::ServerConfig {
        ssh_target: Some("server-vm".to_owned()),
        command: Some("/opt/horizon --server-link".to_owned()),
        ..horizon_core::ServerConfig::default()
    };
    app.load_machines(&ctx);
    settle_machine_load(&mut app);

    let sync = app.server_sync.as_ref().expect("the link of the new entry started");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while sync.state_code() != "away" {
        assert!(
            std::time::Instant::now() < deadline,
            "the first attempt of the link ends: {}",
            sync.state_code()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        app.machine_list.records().len(),
        1,
        "the machine of the person stays in the list while the server is away"
    );
    assert_eq!(
        app.machine_list.find("build-1").map(|machine| machine.host.as_str()),
        Some("203.0.113.10"),
        "and it is the record that this computer wrote"
    );
    assert_eq!(
        app.machine_list.key("build-1"),
        Some("build-1"),
        "the key of the machine stays bound to it"
    );
    let bindings = fs::read_to_string(
        MachineRegistry::from_home(app.session_store.home())
            .root()
            .join("keys.json"),
    )
    .expect("the binding file");
    assert!(
        bindings.contains("\"build-1\": \"build-1\""),
        "a link that never answered does not erase the key binding of this computer: {bindings}"
    );
}
