//! A terminal that a machine opened comes back attached to its tmux session.

use crate::board::panel_restore_options;
use crate::panel::PanelKind;
use crate::runtime_state::PanelState;
use crate::ssh::{MachineKeyRef, SshConnection};

fn ssh_panel(machine: Option<&str>) -> PanelState {
    PanelState {
        local_id: "panel".to_string(),
        name: "build-1".to_string(),
        kind: PanelKind::Ssh,
        ssh_connection: Some(SshConnection {
            host: "203.0.113.10".to_string(),
            remote_command: Some("tmux new -A -s build-1".to_string()),
            machine_key: Some(MachineKeyRef {
                directory: "/home/peters/.horizon/machine-keys".to_string(),
                name: "build-1".to_string(),
            }),
            ..SshConnection::default()
        }),
        machine: machine.map(str::to_string),
        ..PanelState::default()
    }
}

#[test]
fn a_terminal_of_a_machine_reconnects_instead_of_showing_a_frozen_screen() {
    let root = std::path::Path::new("/tmp/transcripts");
    let options = panel_restore_options(
        &ssh_panel(Some("build-1")),
        Some(root),
        &crate::browser::BrowserConfig::default(),
    );
    assert!(
        !options.restore_as_disconnected_snapshot,
        "the saved command attaches to the same tmux session, so the panel runs ssh again"
    );
    assert_eq!(options.machine.as_deref(), Some("build-1"));
    assert_eq!(
        options
            .ssh_connection
            .as_ref()
            .and_then(|connection| connection.remote_command.as_deref()),
        Some("tmux new -A -s build-1"),
        "the panel keeps its own saved connection"
    );
}

#[test]
fn a_terminal_of_no_machine_keeps_the_snapshot_of_its_last_screen() {
    let root = std::path::Path::new("/tmp/transcripts");
    let options = panel_restore_options(&ssh_panel(None), Some(root), &crate::browser::BrowserConfig::default());
    assert!(options.restore_as_disconnected_snapshot);
    assert_eq!(options.machine, None);
}
