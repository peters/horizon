use std::collections::HashMap;
use std::path::PathBuf;

use crate::agents::AgentStatus;
use crate::editor::PanelContent;
use crate::error::Result;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, new_local_id};
use crate::ssh::{SshConnection, SshConnectionStatus};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::transcript::PanelTranscript;
use crate::workspace::WorkspaceId;

use super::super::{
    DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions,
    PanelResume,
};
use super::{
    ResolvedTerminalLaunch, agent_env, current_unix_millis, default_shell, kitty_keyboard_for_kind,
    resolve_terminal_launch, scrollback_limit_for_kind,
};

struct TerminalPanelBuildArgs {
    id: PanelId,
    local_id: String,
    title: String,
    kind: PanelKind,
    resume: PanelResume,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    workspace_id: WorkspaceId,
    session_binding: Option<AgentSessionBinding>,
    template: Option<PanelTemplateRef>,
    has_custom_name: bool,
    launch_command: Option<String>,
    launch_args: Vec<String>,
    launch_cwd: Option<PathBuf>,
    ssh_connection: Option<SshConnection>,
}

pub(in crate::panel) fn restore_failure_panel(
    id: PanelId,
    workspace_id: WorkspaceId,
    opts: PanelOptions,
    error_message: &str,
) -> Result<Panel> {
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);
    let PanelOptions {
        name,
        name_is_custom,
        command,
        args,
        cwd,
        ssh_connection,
        rows,
        cols,
        kind,
        resume,
        position,
        size,
        session_binding,
        template,
        ..
    } = opts;

    let saved_ssh_connection = ssh_connection.clone();
    let has_custom_name = name_is_custom.unwrap_or_else(|| name.is_some());
    let title = name.unwrap_or_else(|| default_terminal_title(id, saved_ssh_connection.as_ref()));
    let replay_bytes = restore_failure_replay_bytes(&title, error_message);
    let terminal = spawn_restore_failure_snapshot_terminal(id, kind, rows, cols, replay_bytes)?;
    let ssh_status = if kind == PanelKind::Ssh {
        Some(SshConnectionStatus::Disconnected)
    } else {
        None
    };

    Ok(build_terminal_panel(
        TerminalPanelBuildArgs {
            id,
            local_id,
            title,
            kind,
            resume,
            position,
            size,
            workspace_id,
            session_binding,
            template,
            has_custom_name,
            launch_command: command,
            launch_args: args,
            launch_cwd: cwd,
            ssh_connection: saved_ssh_connection,
        },
        terminal,
        ssh_status,
    ))
}

pub(super) fn spawn_terminal(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    opts: PanelOptions,
) -> Result<Panel> {
    let PanelOptions {
        name,
        name_is_custom,
        command,
        args,
        cwd,
        ssh_connection,
        rows,
        cols,
        kind,
        resume,
        position,
        size,
        session_binding,
        template,
        transcript_root,
        restore_as_disconnected_snapshot,
        is_restore,
        ..
    } = opts;

    let (transcript, replay_bytes, had_persisted_transcript_state) =
        prepare_transcript_restore(id, kind, transcript_root, &local_id);
    let saved_command = command.clone();
    let saved_args = args.clone();
    let saved_cwd = cwd.clone();
    let saved_ssh_connection = ssh_connection.clone();
    let resolved_launch = resolve_terminal_launch(
        id,
        kind,
        &resume,
        name.as_deref(),
        command,
        args,
        ssh_connection,
        session_binding,
        saved_cwd.as_ref(),
        transcript.as_ref(),
        is_restore,
    );
    let ResolvedTerminalLaunch {
        session_binding,
        program,
        launch_args,
    } = resolved_launch;
    let has_custom_name = name_is_custom.unwrap_or_else(|| name.is_some());
    let title = name.unwrap_or_else(|| default_terminal_title(id, saved_ssh_connection.as_ref()));
    let initial_ssh_status = if kind == PanelKind::Ssh {
        Some(SshConnectionStatus::Connecting)
    } else {
        None
    };
    let env = agent_env(kind, &local_id);
    let panel_args = TerminalPanelBuildArgs {
        id,
        local_id,
        title,
        kind,
        resume,
        position,
        size,
        workspace_id,
        session_binding,
        template,
        has_custom_name,
        launch_command: saved_command,
        launch_args: saved_args,
        launch_cwd: saved_cwd,
        ssh_connection: saved_ssh_connection,
    };
    if restore_as_disconnected_snapshot && panel_args.kind == PanelKind::Ssh && had_persisted_transcript_state {
        return spawn_disconnected_ssh_snapshot_panel(panel_args, rows, cols, replay_bytes);
    }
    let terminal = Terminal::spawn(TerminalSpawnOptions {
        program,
        args: launch_args,
        cwd,
        rows,
        cols,
        cell_width: DEFAULT_CELL_WIDTH,
        cell_height: DEFAULT_CELL_HEIGHT,
        scrollback_limit: scrollback_limit_for_kind(kind),
        window_id: id.0,
        replay_bytes,
        env,
        kitty_keyboard: kitty_keyboard_for_kind(kind),
    })?;
    tracing::info!("created panel '{}' (id={})", panel_args.title, panel_args.id.0);
    Ok(build_terminal_panel(panel_args, terminal, initial_ssh_status))
}

fn spawn_restore_failure_snapshot_terminal(
    id: PanelId,
    kind: PanelKind,
    rows: u16,
    cols: u16,
    replay_bytes: Vec<u8>,
) -> Result<Terminal> {
    let (program, args) = disconnected_snapshot_launch_command();
    Terminal::spawn(TerminalSpawnOptions {
        program,
        args,
        cwd: None,
        rows,
        cols,
        cell_width: DEFAULT_CELL_WIDTH,
        cell_height: DEFAULT_CELL_HEIGHT,
        scrollback_limit: scrollback_limit_for_kind(kind),
        window_id: id.0,
        replay_bytes,
        env: HashMap::new(),
        kitty_keyboard: kitty_keyboard_for_kind(kind),
    })
}

fn restore_failure_replay_bytes(title: &str, error_message: &str) -> Vec<u8> {
    format!(
        concat!(
            "Horizon could not restore this panel.\r\n\r\n",
            "Panel: {title}\r\n",
            "Error: {error_message}\r\n\r\n",
            "Fix the command or binary, then restart the panel.\r\n"
        ),
        title = title,
        error_message = error_message
    )
    .into_bytes()
}

fn spawn_disconnected_snapshot_terminal(id: PanelId, rows: u16, cols: u16, replay_bytes: Vec<u8>) -> Result<Terminal> {
    let (program, args) = disconnected_snapshot_launch_command();
    Terminal::spawn(TerminalSpawnOptions {
        program,
        args,
        cwd: None,
        rows,
        cols,
        cell_width: DEFAULT_CELL_WIDTH,
        cell_height: DEFAULT_CELL_HEIGHT,
        scrollback_limit: scrollback_limit_for_kind(PanelKind::Ssh),
        window_id: id.0,
        replay_bytes,
        env: HashMap::new(),
        kitty_keyboard: kitty_keyboard_for_kind(PanelKind::Ssh),
    })
}

pub(super) fn disconnected_snapshot_launch_command() -> (String, Vec<String>) {
    if cfg!(windows) {
        ("cmd.exe".to_string(), vec!["/C".to_string(), "exit".to_string()])
    } else {
        (default_shell(), vec!["-c".to_string(), "exit".to_string()])
    }
}

fn spawn_disconnected_ssh_snapshot_panel(
    panel_args: TerminalPanelBuildArgs,
    rows: u16,
    cols: u16,
    replay_bytes: Vec<u8>,
) -> Result<Panel> {
    let terminal = spawn_disconnected_snapshot_terminal(panel_args.id, rows, cols, replay_bytes)?;
    tracing::info!(
        "restored disconnected ssh snapshot '{}' (id={})",
        panel_args.title,
        panel_args.id.0
    );
    Ok(build_terminal_panel(
        panel_args,
        terminal,
        Some(SshConnectionStatus::Disconnected),
    ))
}

fn build_terminal_panel(
    panel_args: TerminalPanelBuildArgs,
    terminal: Terminal,
    ssh_status: Option<SshConnectionStatus>,
) -> Panel {
    let TerminalPanelBuildArgs {
        id,
        local_id,
        title,
        kind,
        resume,
        position,
        size,
        workspace_id,
        session_binding,
        template,
        has_custom_name,
        launch_command,
        launch_args,
        launch_cwd,
        ssh_connection,
    } = panel_args;
    Panel {
        id,
        local_id,
        title,
        kind,
        resume,
        layout: PanelLayout {
            position: position.unwrap_or_default(),
            size: size.unwrap_or(DEFAULT_PANEL_SIZE),
        },
        visible: true,
        workspace_id,
        content: PanelContent::Terminal(terminal),
        session_binding,
        template,
        launched_at_millis: current_unix_millis(),
        has_custom_name,
        had_recent_output: false,
        agent_status: AgentStatus::default(),
        last_output_at_millis: None,
        terminal_title: String::new(),
        launch_command,
        launch_args,
        launch_cwd,
        ssh_connection,
        ssh_status,
    }
}

fn default_terminal_title(id: PanelId, ssh_connection: Option<&SshConnection>) -> String {
    ssh_connection.map_or_else(
        || format!("Terminal {}", id.0),
        |connection| format!("SSH: {}", connection.display_label()),
    )
}

pub(super) fn prepare_transcript_restore(
    id: PanelId,
    kind: PanelKind,
    transcript_root: Option<PathBuf>,
    local_id: &str,
) -> (Option<PanelTranscript>, Vec<u8>, bool) {
    let mut transcript = PanelTranscript::for_panel(kind, transcript_root, local_id);
    let had_persisted_state = transcript.as_ref().is_some_and(PanelTranscript::has_persisted_state);
    let replay_bytes = if let Some(active_transcript) = transcript.as_ref() {
        match active_transcript.prepare_replay_bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    panel_id = id.0,
                    kind = ?kind,
                    "failed to prepare persisted transcript, starting fresh shell: {error}"
                );
                transcript = None;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    (transcript, replay_bytes, had_persisted_state)
}
