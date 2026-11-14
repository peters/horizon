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
    PanelStart, PreparedLaunch, ResolvedTerminalLaunch, current_unix_millis, default_shell, kitty_keyboard_for_kind,
    replay_bytes_with_notice, resolve_terminal_launch, scrollback_limit_for_kind, start::StartRequest,
};

struct TerminalPanelBuildArgs {
    id: PanelId,
    local_id: String,
    title: String,
    kind: PanelKind,
    resume: PanelResume,
    work_resume: crate::agent_work::ResumePolicy,
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
    opened_by: Option<String>,
    machine: Option<String>,
    machine_session: Option<String>,
}

/// What a placeholder panel says instead of running its process.
#[derive(Clone, Copy, Debug)]
pub(in crate::panel) enum Placeholder<'a> {
    /// The process could not start; the text is the error.
    RestoreFailure(&'a str),
    /// The panel cannot start its process in its working directory.
    WorkingDirectory { path: &'a str, reason: &'a str },
}

impl<'a> Placeholder<'a> {
    /// What a panel says instead of running in `failure`'s directory.
    pub(in crate::panel) fn for_working_directory(failure: &'a super::UnusableWorkingDirectory) -> Self {
        Self::WorkingDirectory {
            path: &failure.path,
            reason: &failure.reason,
        }
    }
}

pub(in crate::panel) fn placeholder_panel(
    id: PanelId,
    workspace_id: WorkspaceId,
    opts: PanelOptions,
    placeholder: Placeholder<'_>,
) -> Result<Panel> {
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);
    let visible = opts.visible;
    let device_identity = (opts.kind == PanelKind::Device)
        .then(|| opts.device_identity.clone())
        .flatten();
    let browser_profile = (opts.kind == PanelKind::Browser).then(|| crate::runtime_state::BrowserProfileState {
        session_id: opts.browser_session_id.clone(),
        root: opts
            .browser_config
            .as_ref()
            .and_then(|config| config.profile_root.clone()),
        backend: opts.browser_config.as_ref().map(|config| config.backend),
        hidden: !visible,
        remote_target: opts.remote_target.clone(),
        host: opts.browser_host.clone(),
        private: opts.browser_private,
    });
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
        work_resume,
        position,
        size,
        session_binding,
        template,
        ..
    } = opts;

    let saved_ssh_connection = ssh_connection.clone();
    let has_custom_name = name_is_custom.unwrap_or_else(|| name.is_some());
    let title = name.unwrap_or_else(|| default_terminal_title(id, saved_ssh_connection.as_ref()));
    let terminal =
        spawn_restore_failure_snapshot_terminal(id, kind, rows, cols, placeholder_replay_bytes(&title, placeholder))?;
    let ssh_status = if kind == PanelKind::Ssh {
        Some(SshConnectionStatus::Disconnected)
    } else {
        None
    };

    let mut panel = build_terminal_panel(
        TerminalPanelBuildArgs {
            id,
            local_id,
            title,
            kind,
            resume,
            work_resume,
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
            opened_by: None,
            machine: None,
            machine_session: None,
        },
        terminal,
        ssh_status,
    );
    panel.visible = visible;
    panel.disconnected_browser_profile = browser_profile;
    panel.disconnected_device_identity = device_identity;
    Ok(panel)
}

/// Starts a panel whose preparation ran on this thread. A frame uses
/// [`waiting_terminal_panel`] and [`Panel::apply_start`] instead, because the
/// three reads below must not run on the UI thread.
pub(super) fn spawn_terminal(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    opts: PanelOptions,
) -> Result<Panel> {
    let request = StartRequest {
        id,
        local_id: local_id.clone(),
        opts: opts.clone(),
    };
    let start = super::start::prepare_panel_start(request);
    spawn_started_terminal(id, workspace_id, local_id, opts, start)
}

/// The panel of a process that waits for its preparation. It says that Horizon
/// prepares it, and the answer of the worker replaces that screen.
pub(super) fn waiting_terminal_panel(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    opts: PanelOptions,
) -> Result<Panel> {
    let rows = opts.rows;
    let cols = opts.cols;
    let kind = opts.kind;
    let ssh_status = (kind == PanelKind::Ssh).then_some(SshConnectionStatus::Connecting);
    let args = terminal_panel_args(id, workspace_id, local_id, opts);
    // Short lines, so a panel of any width says why it is empty.
    let replay_bytes = concat!(
        "Its saved screen and the launch\r\n",
        "of its process are read in the\r\n",
        "background. Preparing this panel.\r\n",
        "The first output of the panel\r\n",
        "replaces this text.\r\n"
    )
    .as_bytes()
    .to_vec();
    let terminal = spawn_restore_failure_snapshot_terminal(id, kind, rows, cols, replay_bytes)?;
    Ok(build_terminal_panel(args, terminal, ssh_status))
}

/// Builds the terminal fields of a panel from its options, without a read.
fn terminal_panel_args(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    opts: PanelOptions,
) -> TerminalPanelBuildArgs {
    let PanelOptions {
        name,
        name_is_custom,
        rows: _,
        cols: _,
        kind,
        resume,
        work_resume,
        position,
        size,
        template,
        is_restore: _,
        transcript_root: _,
        launch_wrapper: _,
        command,
        args,
        cwd,
        session_binding,
        ssh_connection,
        opened_by,
        machine,
        machine_session,
        ..
    } = opts;
    let (has_custom_name, title) = panel_title(name, name_is_custom, id, ssh_connection.as_ref());
    TerminalPanelBuildArgs {
        id,
        local_id,
        title,
        kind,
        resume,
        work_resume,
        position,
        size,
        workspace_id,
        session_binding,
        template,
        has_custom_name,
        launch_command: command,
        launch_args: args,
        launch_cwd: cwd,
        ssh_connection,
        opened_by,
        machine,
        machine_session,
    }
}

/// Starts the process of a panel with the preparation that a worker read. The
/// process start is the one work of a panel that a frame does itself.
pub(super) fn spawn_started_terminal(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    opts: PanelOptions,
    start: PanelStart,
) -> Result<Panel> {
    let PanelStart {
        working_directory,
        replay_bytes,
        had_persisted_transcript_state,
        launch,
        editor_text: _,
        work_brief_wanted: _,
    } = start;
    let Some(launch) = launch else {
        return Err(crate::error::Error::State("This panel runs no process".into()));
    };
    if let Some(failure) = &working_directory {
        return placeholder_panel(id, workspace_id, opts, Placeholder::for_working_directory(failure));
    }
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
        work_resume,
        position,
        size,
        template,
        restore_as_disconnected_snapshot,
        is_restore: _,
        launch_wrapper,
        opened_by,
        machine,
        machine_session,
        ..
    } = opts;
    let (saved_cwd, saved_ssh_connection) = (cwd.clone(), ssh_connection.clone());
    let PreparedLaunch {
        session_binding,
        program,
        args: launch_args,
        env,
        work,
        named_secrets_late,
    } = launch;
    let replay_bytes = replay_bytes_with_notice(replay_bytes, named_secrets_late);
    let (has_custom_name, title) = panel_title(name, name_is_custom, id, saved_ssh_connection.as_ref());
    let initial_ssh_status = (kind == PanelKind::Ssh).then_some(SshConnectionStatus::Connecting);
    let panel_args = TerminalPanelBuildArgs {
        id,
        local_id,
        title,
        kind,
        resume,
        work_resume,
        position,
        size,
        workspace_id,
        session_binding,
        template,
        has_custom_name,
        launch_command: command,
        launch_args: args,
        launch_cwd: saved_cwd,
        ssh_connection: saved_ssh_connection,
        opened_by,
        machine,
        machine_session,
    };
    if restore_as_disconnected_snapshot && panel_args.kind == PanelKind::Ssh && had_persisted_transcript_state {
        return spawn_disconnected_ssh_snapshot_panel(panel_args, rows, cols, replay_bytes);
    }
    // The wrapper runs the resolved program, so the program starts inside it.
    let (program, launch_args) = match launch_wrapper.split_first() {
        Some((wrapper, wrapper_args)) => {
            let mut args = wrapper_args.to_vec();
            args.push(program);
            args.extend(launch_args);
            (wrapper.clone(), args)
        }
        None => (program, launch_args),
    };
    let mut terminal = Terminal::spawn(TerminalSpawnOptions {
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
    terminal.work_owner = work.owner;
    terminal.work_continuation = work.state;
    tracing::info!("created panel '{}' (id={})", panel_args.title, panel_args.id.0);
    Ok(build_terminal_panel(panel_args, terminal, initial_ssh_status))
}

pub(crate) fn resolve_spawn_launch(
    id: PanelId,
    opts: &PanelOptions,
    transcript: Option<&PanelTranscript>,
) -> ResolvedTerminalLaunch {
    resolve_terminal_launch(
        id,
        opts.kind,
        &opts.resume,
        opts.name.as_deref(),
        opts.command.clone(),
        opts.args.clone(),
        opts.ssh_connection.clone(),
        opts.session_binding.clone(),
        opts.cwd.as_ref(),
        transcript,
        opts.is_restore,
    )
}

fn spawn_restore_failure_snapshot_terminal(
    id: PanelId,
    kind: PanelKind,
    rows: u16,
    cols: u16,
    replay_bytes: Vec<u8>,
) -> Result<Terminal> {
    let (program, args) = disconnected_snapshot_launch_command();
    let mut terminal = Terminal::spawn_snapshot(TerminalSpawnOptions {
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
    })?;
    // Its shell exits at once; the PTY goes with it and the replayed screen stays.
    terminal.release_when_exited();
    Ok(terminal)
}

/// The terminal of a placeholder for `panel` that says `placeholder`.
pub(in crate::panel) fn placeholder_terminal(
    panel: &Panel,
    rows: u16,
    cols: u16,
    placeholder: Placeholder<'_>,
) -> Result<Terminal> {
    spawn_restore_failure_snapshot_terminal(
        panel.id,
        panel.kind,
        rows,
        cols,
        placeholder_replay_bytes(&panel.title, placeholder),
    )
}

fn placeholder_replay_bytes(title: &str, placeholder: Placeholder<'_>) -> Vec<u8> {
    match placeholder {
        Placeholder::RestoreFailure(error_message) => format!(
            concat!(
                "Horizon could not restore this panel.\r\n\r\n",
                "Panel: {title}\r\n",
                "Error: {error_message}\r\n\r\n",
                "Fix the command or binary, then restart the panel.\r\n"
            ),
            title = title,
            error_message = error_message
        ),
        Placeholder::WorkingDirectory { path, reason } => format!(
            concat!(
                "Horizon could not start this panel.\r\n\r\n",
                "Panel: {title}\r\n",
                "Directory: {path}\r\n",
                "Reason: {reason}\r\n\r\n",
                "Set the directory of this workspace, then restart the panel.\r\n"
            ),
            title = title,
            path = path,
            reason = reason
        ),
    }
    .into_bytes()
}

fn spawn_disconnected_snapshot_terminal(id: PanelId, rows: u16, cols: u16, replay_bytes: Vec<u8>) -> Result<Terminal> {
    let (program, args) = disconnected_snapshot_launch_command();
    Terminal::spawn_snapshot(TerminalSpawnOptions {
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
    (default_shell(), vec!["-c".to_string(), "exit".to_string()])
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
        work_resume,
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
        opened_by,
        machine,
        machine_session,
    } = panel_args;
    Panel {
        id,
        local_id,
        title,
        kind,
        resume,
        work_resume,
        layout: PanelLayout {
            position: position.unwrap_or_default(),
            size: size.unwrap_or(DEFAULT_PANEL_SIZE),
        },
        visible: true,
        workspace_id,
        content: PanelContent::Terminal(terminal),
        disconnected_browser_profile: None,
        disconnected_device_identity: None,
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
        cwd_read: None,
        ssh_connection,
        opened_by,
        machine,
        machine_session,
        ssh_status,
        ssh_reconnect_attempt: 0,
        ssh_connected_at_millis: None,
        ssh_exit_recorded: false,
        ssh_end: None,
        program_end: None,
    }
}

/// The title of a new terminal, and whether the person chose it.
fn panel_title(
    name: Option<String>,
    name_is_custom: Option<bool>,
    id: PanelId,
    ssh_connection: Option<&SshConnection>,
) -> (bool, String) {
    (
        name_is_custom.unwrap_or_else(|| name.is_some()),
        name.unwrap_or_else(|| default_terminal_title(id, ssh_connection)),
    )
}

fn default_terminal_title(id: PanelId, ssh_connection: Option<&SshConnection>) -> String {
    ssh_connection.map_or_else(
        || format!("Terminal {}", id.0),
        |connection| format!("SSH: {}", connection.display_label()),
    )
}

pub(crate) fn prepare_transcript_restore(
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
                    "failed to prepare persisted transcript: {error}"
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
