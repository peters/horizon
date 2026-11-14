//! The terminal of a machine, opened by an agent and by a click.

use std::fs;
use std::path::{Path, PathBuf};

use egui::Context;
use horizon_browser_control::manifest::block::{self, Outcome};
use horizon_browser_control::manifest::block_terminal::TerminalRoute;
use horizon_core::browser::manifest;
use horizon_core::{MachineKeys, PanelKind, RuntimeState, StartupDecision};

use super::*;
use crate::app::block_machines::settle_machine_load;
use crate::app::test_support::{editor_workspace_state, test_app_with_startup};

fn app() -> (tempfile::TempDir, Context, HorizonApp) {
    let runtime = RuntimeState {
        workspaces: vec![editor_workspace_state("first", [0.0, 0.0])],
        ..RuntimeState::default()
    };
    let (temp, ctx, mut app) = test_app_with_startup(StartupDecision::Ephemeral {
        runtime_state: Box::new(runtime),
    });
    app.board.panels[0].kind = PanelKind::Claude;
    // The startup read of the machine list answers before a test writes one.
    settle_machine_load(&mut app);
    (temp, ctx, app)
}

fn queue(temp: &tempfile::TempDir) -> PathBuf {
    let root = temp.path().join("block-panel-requests");
    fs::create_dir_all(&root).expect("the queue directory");
    root
}

fn plant_key(app: &HorizonApp, name: &str) {
    let directory = MachineKeys::from_home(app.session_store.home()).key_directory();
    fs::create_dir_all(&directory).expect("the key directory");
    fs::write(directory.join(name), "-----BEGIN OPENSSH PRIVATE KEY-----\n").expect("the private key");
    fs::write(directory.join(format!("{name}.pub")), "ssh-ed25519 AAAA\n").expect("the public key");
}

fn set_machine(name: &str, session: Option<&str>, key: Option<&str>) -> block::Operation {
    block::Operation::MachineSet {
        name: name.into(),
        ssh: TerminalRoute {
            host: "203.0.113.10".into(),
            user: Some("root".into()),
            port: None,
        },
        key: key.map(str::to_owned),
        session: session.map(str::to_owned),
        provider: None,
        repository: None,
    }
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

fn answered(app: &mut HorizonApp, ctx: &Context, root: &Path, operation: block::Operation) -> Outcome {
    let request = request(app, operation);
    assert!(
        app.apply_block_request_in(&request, root, ctx).is_none(),
        "the operation answers from the coordination worker"
    );
    app.settle_browser_host_io();
    block::take_result_at(root, &request)
        .expect("the answer is readable")
        .expect("the worker answered")
}

fn code(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Failed { code, .. } => code,
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn an_agent_terminal_of_a_machine_opens_the_panel_of_the_record() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(
        &mut app,
        &ctx,
        &root,
        set_machine("build-1", Some("work"), Some("build-1")),
    );

    let answer = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::OpenTerminal {
            ssh: None,
            remote_command: None,
            title: None,
            key: None,
            machine: Some("build-1".into()),
        },
    );
    let Outcome::Opened { panel_id } = answer else {
        panic!("expected an opened terminal: {answer:?}");
    };
    let panel = app
        .board
        .panel(app.board.panel_id_by_local_id(&panel_id).expect("the terminal"))
        .expect("the terminal");
    assert_eq!(
        panel.workspace_id, app.board.panels[0].workspace_id,
        "the terminal lands in the caller's workspace"
    );
    assert_eq!(panel.machine(), Some("build-1"), "the panel is tied to the machine");
    assert_eq!(panel.opened_by(), Some("Claude"));
    let args = panel.ssh_connection.as_ref().expect("the connection").to_command_args();
    let directory = MachineKeys::from_home(app.session_store.home())
        .key_directory()
        .display()
        .to_string();
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
            "-t".to_string(),
            "root@203.0.113.10".to_string(),
            "tmux new-session -A -s work \\; set-option status off".to_string(),
        ],
        "the agent's terminal is the argv of the record, with the bounds of a machine"
    );
    let listed = answered(&mut app, &ctx, &root, block::Operation::Machines);
    assert!(
        matches!(&listed, Outcome::Machines { machines, .. } if machines[0].terminal_open),
        "the machine reports its open terminal: {listed:?}"
    );
}

#[test]
fn a_machine_terminal_with_another_field_is_refused_naming_the_field() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);
    plant_key(&app, "build-1");
    answered(&mut app, &ctx, &root, set_machine("build-1", None, Some("build-1")));
    let before = app.board.panels.len();

    for (field, operation) in [
        (
            "ssh",
            block::Operation::OpenTerminal {
                ssh: Some(TerminalRoute {
                    host: "203.0.113.99".into(),
                    user: None,
                    port: None,
                }),
                remote_command: None,
                title: None,
                key: None,
                machine: Some("build-1".into()),
            },
        ),
        (
            "key",
            block::Operation::OpenTerminal {
                ssh: None,
                remote_command: None,
                title: None,
                key: Some("build-1".into()),
                machine: Some("build-1".into()),
            },
        ),
        (
            "remote_command",
            block::Operation::OpenTerminal {
                ssh: None,
                remote_command: Some("tmux new -A -s work".into()),
                title: None,
                key: None,
                machine: Some("build-1".into()),
            },
        ),
    ] {
        let request = request(&app, operation);
        let refused = app.apply_block_request(&request, &ctx);
        assert_eq!(code(&refused), "invalid_request", "{field} must be refused");
        assert!(
            format!("{refused:?}").contains(field),
            "the refusal names {field}: {refused:?}"
        );
    }
    assert_eq!(app.board.panels.len(), before, "a refused field opens no panel");

    let unknown = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::OpenTerminal {
            ssh: None,
            remote_command: None,
            title: None,
            key: None,
            machine: Some("build-9".into()),
        },
    );
    assert_eq!(code(&unknown), "machine_unavailable");
}

#[test]
fn an_agent_terminal_that_opens_nothing_says_nothing_to_the_person() {
    let (temp, ctx, mut app) = app();
    let root = queue(&temp);

    let answer = answered(
        &mut app,
        &ctx,
        &root,
        block::Operation::OpenTerminal {
            ssh: None,
            remote_command: None,
            title: None,
            key: None,
            machine: Some("forgotten-machine".into()),
        },
    );

    assert!(
        matches!(&answer, Outcome::Failed { code, .. } if code == "machine_unavailable"),
        "the agent reads its own typed failure: {answer:?}"
    );
    assert_eq!(
        app.sidebar_notice, None,
        "an agent's create answers the agent, so the person gets no notice of it"
    );
}
