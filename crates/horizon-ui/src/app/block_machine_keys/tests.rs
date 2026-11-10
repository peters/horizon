//! The machine keys an agent asks Horizon to keep, judged on a test board.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use egui::Context;
use horizon_browser_control::manifest::block::{self, Outcome};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::browser::manifest;
use horizon_core::{
    CanvasViewState, MachineKeyRef, MachineKeys, PanelKind, RuntimeState, SshConnection, StartupDecision, WindowConfig,
};

use super::*;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
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
    // No live agent process is needed to exercise the caller's identity.
    app.board.panels[0].kind = PanelKind::Claude;
    app.board.panels[1].kind = PanelKind::Claude;
    (temp, ctx, app)
}

/// The queue directory of a test, so no answer reaches the runtime root of the
/// person running the test.
fn queue(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("block-panel-requests");
    fs::create_dir_all(&root).expect("the queue directory");
    root
}

fn request_from(app: &HorizonApp, panel: usize, operation: block::Operation) -> block::Request {
    block::Request {
        request_id: "test".into(),
        actor: format!("horizon:{}", app.board.panels[panel].local_id),
        host_instance: manifest::host_instance().into(),
        deadline_at_millis: manifest::now_millis() + 30_000,
        operation,
    }
}

fn request(app: &HorizonApp, operation: block::Operation) -> block::Request {
    request_from(app, 0, operation)
}

fn open_terminal(key: Option<&str>) -> block::Operation {
    block::Operation::OpenTerminal {
        ssh: Some(TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        }),
        remote_command: Some("tmux new -A -s build".into()),
        title: None,
        key: key.map(str::to_string),
        machine: None,
    }
}

/// The key directory of this Horizon home, as the store resolves it.
fn key_directory(app: &HorizonApp) -> PathBuf {
    MachineKeys::from_home(app.session_store.home()).key_directory()
}

/// Writes a key pair of `name` where the store looks for it, so a test of the
/// terminal needs no `ssh-keygen`.
fn plant_key(app: &HorizonApp, name: &str) -> MachineKeyRef {
    let directory = key_directory(app);
    fs::create_dir_all(&directory).expect("the key directory");
    fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the private key");
    fs::write(
        directory.join(format!("{name}.pub")),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI horizon-machine-build-1\n",
    )
    .expect("the public key");
    MachineKeyRef {
        directory: directory.display().to_string(),
        name: name.to_owned(),
    }
}

/// Applies one request of the first agent panel that the coordination worker
/// answers, and returns its answer as the asking agent reads it.
fn answered(app: &mut HorizonApp, ctx: &Context, root: &Path, operation: block::Operation) -> Outcome {
    let request = request_from(app, 0, operation);
    assert!(
        app.apply_block_request_in(&request, root, ctx).is_none(),
        "a key operation answers from the coordination worker"
    );
    app.settle_browser_host_io();
    block::take_result_at(root, &request)
        .expect("the answer is readable")
        .expect("the worker answered")
}

fn opened(outcome: Outcome) -> String {
    match outcome {
        Outcome::Opened { panel_id } => panel_id,
        other => panic!("expected an opened terminal: {other:?}"),
    }
}

fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

fn ssh_keygen_available() -> bool {
    match Command::new("ssh-keygen").arg("-?").output() {
        Ok(_) => true,
        Err(error) => {
            eprintln!("skipping: this test machine has no ssh-keygen: {error}");
            false
        }
    }
}

fn expected_args(key: &MachineKeyRef) -> Vec<String> {
    let directory = key.directory.clone();
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
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=1".to_string(),
        "-t".to_string(),
        "root@203.0.113.10".to_string(),
        "tmux new -A -s build".to_string(),
    ]
}

#[test]
fn a_terminal_with_a_machine_key_connects_with_that_key_and_carries_its_name() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let key = plant_key(&app, "build-1");

    let panel_id = opened(answered(&mut app, &ctx, &root, open_terminal(Some("build-1"))));
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("the terminal"))
        .expect("the terminal");
    assert_eq!(
        panel.workspace_id, app.board.panels[0].workspace_id,
        "the terminal lands in the caller's workspace"
    );
    assert_eq!(panel.opened_by(), Some("Claude"));
    let connection = panel.ssh_connection.as_ref().expect("the connection");
    assert_eq!(
        connection.machine_key.as_ref(),
        Some(&key),
        "the panel carries the key that it connects with"
    );
    assert_eq!(connection.identity_file, None, "an agent passes no identity file");
    assert_eq!(connection.to_command_args(), expected_args(&key));
}

#[test]
fn a_key_that_this_home_lacks_is_refused_without_a_panel() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let before = app.board.panels.len();

    let refusal = answered(&mut app, &ctx, &root, open_terminal(Some("build-9")));
    assert_eq!(code(&refusal), "key_unavailable");
    assert_eq!(app.board.panels.len(), before, "a missing key opens no panel");
    assert!(
        format!("{refusal:?}").contains("machine_key"),
        "the refusal names the operation that makes one: {refusal:?}"
    );
}

#[test]
fn a_key_of_one_workspace_is_offered_to_an_agent_panel_of_another() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    // The pair is in the Horizon home; the first workspace's agent asks for it.
    plant_key(&app, "build-1");
    let before = app.board.panels.len();

    let panel_id = opened(answered(&mut app, &ctx, &root, open_terminal(Some("build-1"))));
    assert_eq!(
        app.board.panels.len(),
        before + 1,
        "the keys belong to the person's Horizon home, not to one workspace"
    );
    assert!(app.board.panel_id_by_local_id(&panel_id).is_some());

    let listed = answered(&mut app, &ctx, &root, block::Operation::MachineKeys);
    assert!(
        matches!(&listed, Outcome::MachineKeys { keys } if keys.len() == 1 && keys[0].name == "build-1"),
        "every agent panel of this Horizon lists the pair: {listed:?}"
    );
}

#[test]
fn the_key_operations_make_list_and_delete_one_key() {
    if !ssh_keygen_available() {
        return;
    }
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);

    let made = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKey { name: "build-1".into() },
    );
    let Outcome::MachineKey {
        name,
        public_key,
        private_key_path,
        known_hosts_path,
        created,
    } = made
    else {
        panic!("expected a key pair: {made:?}");
    };
    assert_eq!(name, "build-1");
    assert!(created, "the first request makes the pair");
    assert!(public_key.starts_with("ssh-ed25519 "), "{public_key}");
    assert!(
        public_key.contains("horizon-machine-build-1"),
        "the public key names the machine: {public_key}"
    );
    let directory = key_directory(&app);
    assert_eq!(
        Path::new(&private_key_path),
        directory.join("build-1"),
        "the answer names the private half as an absolute path"
    );
    assert!(Path::new(&private_key_path).is_file(), "{private_key_path}");
    assert_eq!(
        Path::new(&known_hosts_path),
        directory.join("build-1.known_hosts"),
        "the answer names the known-hosts file of that key"
    );

    let listed = answered(&mut app, &ctx, &root, block::Operation::MachineKeys);
    let Outcome::MachineKeys { keys } = listed else {
        panic!("expected a list: {listed:?}");
    };
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "build-1");
    assert_eq!(keys[0].public_key, public_key);
    assert_eq!(keys[0].private_key_path, private_key_path);
    assert_eq!(keys[0].known_hosts_path, known_hosts_path);
    assert!(keys[0].created_at_millis > 0);

    // The terminal connects with the pair that the worker made, without a
    // second key file.
    let terminal = answered(&mut app, &ctx, &root, open_terminal(Some("build-1")));
    assert!(matches!(terminal, Outcome::Opened { .. }), "{terminal:?}");

    let deleted = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::DeleteMachineKey { name: "build-1".into() },
    );
    assert!(matches!(&deleted, Outcome::DeletedMachineKey { name } if name == "build-1"));
    let listed = answered(&mut app, &ctx, &root, block::Operation::MachineKeys);
    assert!(
        matches!(&listed, Outcome::MachineKeys { keys } if keys.is_empty()),
        "a deleted key is gone: {listed:?}"
    );
    let refused = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::DeleteMachineKey { name: "build-1".into() },
    );
    assert_eq!(code(&refused), "key_unavailable");
}

#[test]
fn a_restored_panel_builds_the_command_line_of_its_key() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let key = plant_key(&app, "build-1");
    let panel_id = opened(answered(&mut app, &ctx, &root, open_terminal(Some("build-1"))));
    let live = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("the terminal"))
        .and_then(|panel| panel.ssh_connection.clone())
        .expect("the connection")
        .to_command_args();

    let saved = RuntimeState::from_board(&app.board, WindowConfig::default(), CanvasViewState::default());
    let saved_panel = saved
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panels.iter())
        .find(|panel| panel.kind == PanelKind::Ssh)
        .expect("the saved SSH panel");
    assert_eq!(
        saved_panel
            .ssh_connection
            .as_ref()
            .and_then(|connection| connection.machine_key.as_ref())
            .map(|key| key.name.as_str()),
        Some("build-1"),
        "the key name is saved with the panel"
    );
    assert_eq!(
        saved_panel
            .to_panel_options(&horizon_core::browser::BrowserConfig::default())
            .ssh_connection
            .as_ref()
            .map(SshConnection::to_command_args),
        Some(live.clone()),
        "the restore path builds the same command line"
    );

    // A session file carries the connection through JSON, as a restart does.
    let encoded = serde_json::to_string(&saved).expect("serialize the state");
    let restored: RuntimeState = serde_json::from_str(&encoded).expect("parse the state");
    let restored_args = restored
        .workspaces
        .iter()
        .flat_map(|workspace| workspace.panels.iter())
        .find(|panel| panel.kind == PanelKind::Ssh)
        .and_then(|panel| panel.ssh_connection.clone())
        .map(|connection| connection.to_command_args());
    assert_eq!(restored_args, Some(live), "the same key, the same argv");
    assert_eq!(
        restored
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.panels.iter())
            .find(|panel| panel.kind == PanelKind::Ssh)
            .and_then(|panel| panel.ssh_connection.as_ref())
            .and_then(|connection| connection.machine_key.as_ref()),
        Some(&key)
    );
}

#[test]
fn a_key_operation_of_an_unknown_caller_answers_caller_unavailable() {
    let (_temp, ctx, mut app) = app();
    let mut unknown_caller = request(&app, block::Operation::MachineKeys);
    unknown_caller.actor = "horizon:not-a-panel-here".into();
    assert_eq!(
        code(&app.apply_block_request(&unknown_caller, &ctx)),
        "caller_unavailable",
        "a caller without its agent panel never reaches a key"
    );

    let mut other_host = request(&app, block::Operation::MachineKey { name: "build-1".into() });
    other_host.host_instance = "another-host".into();
    assert_eq!(
        code(&app.apply_block_request(&other_host, &ctx)),
        "wrong_host",
        "a request of another host never reaches a key"
    );
}

#[test]
fn a_key_name_that_breaks_the_rule_is_refused_before_any_file_work() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let refused = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKey {
            name: "../id_ed25519".into(),
        },
    );
    assert_eq!(code(&refused), "invalid_key_name");
    assert!(
        !MachineKeys::from_home(app.session_store.home())
            .key_directory()
            .exists(),
        "a refused name writes no key"
    );
}

#[test]
fn no_key_answer_or_error_holds_the_text_of_the_private_key() {
    if !ssh_keygen_available() {
        return;
    }
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    let made = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKey { name: "build-1".into() },
    );
    let Outcome::MachineKey {
        private_key_path,
        known_hosts_path,
        ..
    } = &made
    else {
        panic!("expected a key pair: {made:?}");
    };
    assert!(
        private_key_path.starts_with(&temp.path().display().to_string()),
        "the pair is in the home of this test: {private_key_path}"
    );
    assert_eq!(
        Path::new(private_key_path).with_file_name("build-1.known_hosts"),
        Path::new(known_hosts_path),
        "the known-hosts file sits beside the pair"
    );
    let private = fs::read_to_string(private_key_path).expect("the private key");
    assert!(
        private.contains("BEGIN OPENSSH PRIVATE KEY"),
        "the answer names a real private key: {private_key_path}"
    );
    let body: Vec<&str> = private
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("-----"))
        .collect();
    assert!(!body.is_empty(), "the private key has a body");

    let mut answers: Vec<String> = vec![serialized(&made)];
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKey { name: "build-1".into() },
    )));
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKeys,
    )));
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        open_terminal(Some("build-1")),
    )));
    // An incomplete pair answers a message of its own.
    let directory = key_directory(&app);
    fs::remove_file(directory.join("build-1.pub")).expect("the public half");
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKey { name: "build-1".into() },
    )));
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::DeleteMachineKey { name: "build-1".into() },
    )));
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::DeleteMachineKey { name: "build-1".into() },
    )));
    answers.push(serialized(&answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::MachineKey {
            name: "../id_ed25519".into(),
        },
    )));

    for answer in &answers {
        for line in &body {
            assert!(
                !answer.contains(line),
                "no answer or error holds the private key text, but this one holds {line}"
            );
        }
    }
}

/// One answer as the asking agent reads it: its debug form and its JSON.
fn serialized(outcome: &Outcome) -> String {
    format!(
        "{outcome:?} {}",
        serde_json::to_string(outcome).expect("serialize the answer")
    )
}
