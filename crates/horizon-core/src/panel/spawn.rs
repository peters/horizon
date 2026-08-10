use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::agent_definition;
use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::git_changes::DiffViewer;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, claude_session_transcript_exists, new_local_id};
use crate::ssh::{SshConnection, SshConnectionStatus};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::transcript::PanelTranscript;
use crate::usage_dashboard::UsageDashboard;
use crate::workspace::WorkspaceId;

#[cfg(test)]
use super::agent_launch::{
    AGENT_LAUNCH_HELPER_ARG, platform_default_shell, resolve_default_shell, shell_escape, windows_setup_agent_command,
    wrap_agent_command,
};
use super::agent_launch::{
    AgentLaunchContext, default_shell, resolve_agent_launch_command, validate_agent_launch_options,
};
use super::{
    AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, DEFAULT_PANEL_SCROLLBACK_LIMIT,
    DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume,
};

struct StaticPanelSeed {
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    name: Option<String>,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    template: Option<PanelTemplateRef>,
}

struct TerminalLaunchTrace<'a> {
    kind: PanelKind,
    resume: &'a PanelResume,
    session_binding: Option<&'a AgentSessionBinding>,
    should_resume_binding: bool,
    details: TerminalLaunchDetails<'a>,
}

#[derive(Debug, PartialEq, Eq)]
enum TerminalLaunchDetails<'a> {
    Redacted,
    Visible { cwd: Option<&'a str>, cmd: String },
}

struct ResolvedTerminalLaunch {
    session_binding: Option<AgentSessionBinding>,
    program: String,
    launch_args: Vec<String>,
}

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
    agent_login_shell: bool,
    launch_cwd: Option<PathBuf>,
    ssh_connection: Option<SshConnection>,
}

impl StaticPanelSeed {
    fn new(
        id: PanelId,
        workspace_id: WorkspaceId,
        local_id: String,
        name: Option<String>,
        position: Option<[f32; 2]>,
        size: Option<[f32; 2]>,
        template: Option<PanelTemplateRef>,
    ) -> Self {
        Self {
            id,
            workspace_id,
            local_id,
            name,
            position,
            size,
            template,
        }
    }

    fn take_title(&mut self, fallback: impl FnOnce() -> String) -> (String, bool) {
        let has_custom_name = self.name.is_some();
        (self.name.take().unwrap_or_else(fallback), has_custom_name)
    }

    fn into_panel(
        self,
        title: String,
        kind: PanelKind,
        content: PanelContent,
        launch_command: Option<String>,
        launch_cwd: Option<PathBuf>,
        has_custom_name: bool,
    ) -> Panel {
        Panel {
            id: self.id,
            local_id: self.local_id,
            title,
            terminal_title: String::new(),
            kind,
            resume: PanelResume::Fresh,
            layout: PanelLayout {
                position: self.position.unwrap_or_default(),
                size: self.size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            workspace_id: self.workspace_id,
            content,
            session_binding: None,
            template: self.template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            had_recent_output: false,
            last_output_at_millis: None,
            launch_command,
            launch_args: Vec::new(),
            agent_login_shell: false,
            launch_cwd,
            ssh_connection: None,
            ssh_status: None,
        }
    }
}

pub(super) fn spawn_panel(id: PanelId, workspace_id: WorkspaceId, opts: PanelOptions) -> Result<Panel> {
    validate_agent_launch_options(&opts)?;
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);

    match opts.kind {
        PanelKind::Editor => {
            let PanelOptions {
                name,
                command,
                position,
                size,
                template,
                ..
            } = opts;
            let seed = StaticPanelSeed::new(id, workspace_id, local_id, name, position, size, template);
            spawn_editor(seed, command)
        }
        PanelKind::GitChanges => {
            let PanelOptions {
                name,
                position,
                size,
                template,
                cwd,
                ..
            } = opts;
            let seed = StaticPanelSeed::new(id, workspace_id, local_id, name, position, size, template);
            Ok(spawn_git_changes(seed, cwd))
        }
        PanelKind::Usage => {
            let PanelOptions {
                name,
                position,
                size,
                template,
                ..
            } = opts;
            let seed = StaticPanelSeed::new(id, workspace_id, local_id, name, position, size, template);
            Ok(spawn_usage(seed))
        }
        _ => spawn_terminal(id, workspace_id, local_id, opts),
    }
}

pub(super) fn restore_failure_panel(
    id: PanelId,
    workspace_id: WorkspaceId,
    opts: PanelOptions,
    error_message: &str,
) -> Result<Panel> {
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);
    let PanelOptions {
        name,
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
        agent_login_shell,
        ..
    } = opts;

    let saved_ssh_connection = ssh_connection.clone();
    let has_custom_name = name.is_some();
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
            agent_login_shell,
            launch_cwd: cwd,
            ssh_connection: saved_ssh_connection,
        },
        terminal,
        ssh_status,
    ))
}

fn spawn_terminal(id: PanelId, workspace_id: WorkspaceId, local_id: String, opts: PanelOptions) -> Result<Panel> {
    let PanelOptions {
        name,
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
        initial_agent_prompt,
        agent_login_shell,
        restore_as_disconnected_snapshot,
        is_restore,
        ..
    } = opts;

    let (transcript, replay_bytes, had_persisted_transcript_state) =
        prepare_transcript_launch(id, kind, transcript_root, &local_id, initial_agent_prompt.is_some());
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
        initial_agent_prompt.as_deref(),
        saved_cwd.as_ref(),
        transcript.as_ref(),
        agent_login_shell,
        is_restore,
    );
    let ResolvedTerminalLaunch {
        session_binding,
        program,
        launch_args,
    } = resolved_launch;
    let has_custom_name = name.is_some();
    let title = name.unwrap_or_else(|| default_terminal_title(id, saved_ssh_connection.as_ref()));
    let initial_ssh_status = if kind == PanelKind::Ssh {
        Some(SshConnectionStatus::Connecting)
    } else {
        None
    };
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
        agent_login_shell,
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
        env: agent_env(kind),
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

fn disconnected_snapshot_launch_command() -> (String, Vec<String>) {
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

#[expect(
    clippy::too_many_arguments,
    reason = "terminal launch resolution needs the saved runtime-state metadata plus transcript context"
)]
fn resolve_terminal_launch(
    id: PanelId,
    kind: PanelKind,
    resume: &PanelResume,
    name: Option<&str>,
    command: Option<String>,
    args: Vec<String>,
    ssh_connection: Option<SshConnection>,
    session_binding: Option<AgentSessionBinding>,
    initial_agent_prompt: Option<&str>,
    saved_cwd: Option<&PathBuf>,
    transcript: Option<&PanelTranscript>,
    agent_login_shell: bool,
    is_restore: bool,
) -> ResolvedTerminalLaunch {
    let saved_cwd_string = saved_cwd.map(|path| path.display().to_string());
    let (session_binding, should_resume_binding) = resolve_session_binding(
        kind,
        resume,
        session_binding,
        saved_cwd_string.as_deref(),
        name,
        claude_session_transcript_exists,
    );
    let (program, launch_args) = resolve_launch_command(
        command,
        args,
        ssh_connection,
        kind,
        AgentLaunchContext {
            resume,
            session_binding: session_binding.as_ref(),
            should_resume_binding,
            initial_agent_prompt,
            agent_login_shell,
            is_restore,
        },
    );

    let launch_trace = TerminalLaunchTrace {
        kind,
        resume,
        session_binding: session_binding.as_ref(),
        should_resume_binding,
        details: terminal_launch_details(
            saved_cwd_string.as_deref(),
            &program,
            &launch_args,
            initial_agent_prompt.is_some() || agent_login_shell,
        ),
    };
    log_terminal_launch(id, &launch_trace);

    let (program, launch_args) = if let Some(transcript) = transcript {
        transcript.wrap_launch_command(program, launch_args)
    } else {
        (program, launch_args)
    };

    ResolvedTerminalLaunch {
        session_binding,
        program,
        launch_args,
    }
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
        agent_login_shell,
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
        workspace_id,
        content: PanelContent::Terminal(terminal),
        session_binding,
        template,
        launched_at_millis: current_unix_millis(),
        has_custom_name,
        had_recent_output: false,
        last_output_at_millis: None,
        terminal_title: String::new(),
        launch_command,
        launch_args,
        agent_login_shell,
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

fn terminal_launch_details<'a>(
    cwd: Option<&'a str>,
    program: &str,
    launch_args: &[String],
    redact: bool,
) -> TerminalLaunchDetails<'a> {
    if redact {
        TerminalLaunchDetails::Redacted
    } else {
        TerminalLaunchDetails::Visible {
            cwd,
            cmd: format!("{program} {}", launch_args.join(" ")),
        }
    }
}

fn log_terminal_launch(id: PanelId, trace: &TerminalLaunchTrace<'_>) {
    if !trace.kind.is_agent() {
        return;
    }

    match &trace.details {
        TerminalLaunchDetails::Redacted => {
            tracing::info!(
                panel_id = id.0,
                kind = ?trace.kind,
                resume = ?trace.resume,
                session_id = trace.session_binding.map(|binding| binding.session_id.as_str()),
                should_resume = trace.should_resume_binding,
                "launching agent panel (command details redacted)"
            );
        }
        TerminalLaunchDetails::Visible { cwd, cmd } => {
            tracing::info!(
                panel_id = id.0,
                kind = ?trace.kind,
                resume = ?trace.resume,
                session_id = trace.session_binding.map(|binding| binding.session_id.as_str()),
                should_resume = trace.should_resume_binding,
                cwd,
                cmd = %cmd,
                "launching agent panel"
            );
        }
    }
}

fn spawn_editor(mut seed: StaticPanelSeed, command: Option<String>) -> Result<Panel> {
    let editor = if let Some(ref path_str) = command {
        let path = PathBuf::from(path_str);
        if path.exists() {
            MarkdownEditor::open(path)?
        } else {
            let mut editor = MarkdownEditor::scratch();
            editor.file_path = Some(path);
            editor
        }
    } else {
        MarkdownEditor::scratch()
    };

    let (title, has_custom_name) = seed.take_title(|| {
        command
            .as_deref()
            .and_then(|path| {
                PathBuf::from(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
            })
            .unwrap_or_else(|| "Markdown".to_string())
    });

    tracing::info!("created editor panel '{}' (id={})", title, seed.id.0);

    Ok(seed.into_panel(
        title,
        PanelKind::Editor,
        PanelContent::Editor(editor),
        command,
        None,
        has_custom_name,
    ))
}

fn spawn_git_changes(mut seed: StaticPanelSeed, cwd: Option<PathBuf>) -> Panel {
    let (title, has_custom_name) = seed.take_title(|| "Git Changes".to_string());
    tracing::info!("created git changes panel '{}' (id={})", title, seed.id.0);

    seed.into_panel(
        title,
        PanelKind::GitChanges,
        PanelContent::GitChanges(DiffViewer::new()),
        None,
        cwd,
        has_custom_name,
    )
}

fn spawn_usage(mut seed: StaticPanelSeed) -> Panel {
    let (title, has_custom_name) = seed.take_title(|| "Usage".to_string());
    tracing::info!("created usage panel '{}' (id={})", title, seed.id.0);

    seed.into_panel(
        title,
        PanelKind::Usage,
        PanelContent::Usage(UsageDashboard::new()),
        None,
        None,
        has_custom_name,
    )
}

pub(super) fn resolve_launch_command(
    command: Option<String>,
    args: Vec<String>,
    ssh_connection: Option<SshConnection>,
    kind: PanelKind,
    launch: AgentLaunchContext<'_>,
) -> (String, Vec<String>) {
    match kind {
        PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => (String::new(), Vec::new()),
        PanelKind::Shell => {
            let use_login_shell = command.is_none() && PLATFORM_USES_LOGIN_SHELL;
            let program = command.unwrap_or_else(default_shell);
            (program, shell_launch_args(args, use_login_shell))
        }
        PanelKind::Ssh => ssh_connection.map_or_else(
            || (command.unwrap_or_else(|| "ssh".to_string()), args),
            |connection| ("ssh".to_string(), connection.to_command_args()),
        ),
        PanelKind::Command => {
            if let Some(program) = command {
                (program, args)
            } else {
                (default_shell(), args)
            }
        }
        PanelKind::Codex
        | PanelKind::Claude
        | PanelKind::OpenCode
        | PanelKind::Gemini
        | PanelKind::KiloCode
        | PanelKind::Pi => resolve_agent_launch_command(command, args, kind, launch),
    }
}

pub fn current_unix_millis() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(now).unwrap_or(i64::MAX)
}

fn prepare_transcript_restore(
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

fn prepare_transcript_launch(
    id: PanelId,
    kind: PanelKind,
    transcript_root: Option<PathBuf>,
    local_id: &str,
    has_initial_agent_prompt: bool,
) -> (Option<PanelTranscript>, Vec<u8>, bool) {
    if has_initial_agent_prompt {
        return (None, Vec::new(), false);
    }

    prepare_transcript_restore(id, kind, transcript_root, local_id)
}

fn resolve_session_binding(
    kind: PanelKind,
    resume: &PanelResume,
    mut session_binding: Option<AgentSessionBinding>,
    cwd: Option<&str>,
    label: Option<&str>,
    transcript_exists: impl Fn(&str) -> bool,
) -> (Option<AgentSessionBinding>, bool) {
    let had_existing_session_binding = session_binding.is_some();
    if session_binding.is_none() {
        session_binding = match (resume, kind) {
            (PanelResume::Session { session_id }, kind) if kind.supports_session_binding() => {
                Some(AgentSessionBinding::new(
                    kind,
                    session_id.clone(),
                    cwd.map(str::to_string),
                    label.map(str::to_string),
                    None,
                ))
            }
            // Claude accepts a caller-chosen id for fresh launches, so the
            // panel is bound to its session before the CLI writes any session
            // record; a restart then resumes exactly this panel's
            // conversation instead of guessing from catalog timestamps.
            (PanelResume::Fresh | PanelResume::Last, PanelKind::Claude) => Some(AgentSessionBinding::new(
                kind,
                Uuid::new_v4().to_string(),
                cwd.map(str::to_string),
                label.map(str::to_string),
                Some(current_unix_millis()),
            )),
            _ => None,
        };
    }

    let mut should_resume_binding = if kind == PanelKind::Claude {
        session_binding.is_some()
            && (had_existing_session_binding || matches!(resume, PanelResume::Last | PanelResume::Session { .. }))
    } else {
        session_binding.is_some() || matches!(resume, PanelResume::Session { .. })
    };

    // Claude refuses `--resume` for ids without an on-disk transcript (a
    // bound panel that never received a message) and `--session-id` for ids
    // that already have one, so the launch mode follows the store: resume
    // when the transcript exists, otherwise relaunch fresh under the same id.
    if kind == PanelKind::Claude
        && should_resume_binding
        && let Some(binding) = &session_binding
        && !transcript_exists(&binding.session_id)
    {
        should_resume_binding = false;
    }

    (session_binding, should_resume_binding)
}

fn shell_launch_args(args: Vec<String>, use_login_shell: bool) -> Vec<String> {
    if use_login_shell && args.is_empty() {
        vec!["-l".to_string()]
    } else {
        args
    }
}

const PLATFORM_USES_LOGIN_SHELL: bool = cfg!(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
));

pub(super) fn agent_env(kind: PanelKind) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if kind.is_agent() {
        env.insert("HORIZON".to_string(), "1".to_string());
    }
    env
}

pub(super) fn scrollback_limit_for_kind(kind: PanelKind) -> usize {
    if kind.is_agent() {
        AGENT_PANEL_SCROLLBACK_LIMIT
    } else {
        match kind {
            PanelKind::Shell | PanelKind::Ssh | PanelKind::Command => DEFAULT_PANEL_SCROLLBACK_LIMIT,
            PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => 0,
            PanelKind::Codex
            | PanelKind::Claude
            | PanelKind::OpenCode
            | PanelKind::Gemini
            | PanelKind::KiloCode
            | PanelKind::Pi => unreachable!(),
        }
    }
}

pub(super) fn kitty_keyboard_for_kind(kind: PanelKind) -> bool {
    agent_definition(kind).is_none_or(|definition| definition.kitty_keyboard)
}

#[cfg(test)]
mod tests;
