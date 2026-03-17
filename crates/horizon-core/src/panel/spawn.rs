use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::horizon_home::HorizonHome;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, new_local_id};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::transcript::PanelTranscript;
use crate::usage_dashboard::UsageDashboard;
use crate::workspace::WorkspaceId;

use super::{
    AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_CELL_HEIGHT, DEFAULT_CELL_WIDTH, DEFAULT_PANEL_SCROLLBACK_LIMIT,
    DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume,
};

pub(super) fn spawn_panel(id: PanelId, workspace_id: WorkspaceId, opts: PanelOptions) -> Result<Panel> {
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
            spawn_editor(id, workspace_id, local_id, name, command, position, size, template)
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
            spawn_git_changes(id, workspace_id, local_id, name, position, size, template, cwd)
        }
        PanelKind::Usage => {
            let PanelOptions {
                name,
                position,
                size,
                template,
                ..
            } = opts;
            spawn_usage(id, workspace_id, local_id, name, position, size, template)
        }
        _ => spawn_terminal(id, workspace_id, local_id, opts),
    }
}

fn spawn_terminal(id: PanelId, workspace_id: WorkspaceId, local_id: String, opts: PanelOptions) -> Result<Panel> {
    let PanelOptions {
        name,
        command,
        args,
        cwd,
        rows,
        cols,
        kind,
        resume,
        position,
        size,
        session_binding,
        template,
        transcript_root,
        ..
    } = opts;

    let (transcript, replay_bytes) = prepare_transcript_restore(id, kind, transcript_root, &local_id);
    let saved_command = command.clone();
    let saved_args = args.clone();
    let saved_cwd = cwd.clone();
    let saved_cwd_string = saved_cwd.as_ref().map(|path| path.display().to_string());
    let (session_binding, should_resume_binding) = resolve_session_binding(
        kind,
        &resume,
        session_binding,
        saved_cwd_string.as_deref(),
        name.as_deref(),
    );
    let (program, launch_args) = resolve_launch_command(
        command,
        args,
        kind,
        &resume,
        session_binding.as_ref(),
        should_resume_binding,
    );

    if kind.is_agent() {
        tracing::info!(
            panel_id = id.0,
            kind = ?kind,
            resume = ?resume,
            session_id = session_binding.as_ref().map(|binding| binding.session_id.as_str()),
            should_resume = should_resume_binding,
            cwd = saved_cwd_string.as_deref(),
            cmd = %format!("{program} {}", launch_args.join(" ")),
            "launching agent panel"
        );
    }

    let (program, launch_args) = if let Some(transcript) = transcript.as_ref() {
        transcript.wrap_launch_command(program, launch_args)
    } else {
        (program, launch_args)
    };
    let has_custom_name = name.is_some();
    let title = name.unwrap_or_else(|| format!("Terminal {}", id.0));
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

    tracing::info!("created panel '{}' (id={})", title, id.0);

    Ok(Panel {
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
        launch_command: saved_command,
        launch_args: saved_args,
        launch_cwd: saved_cwd,
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_editor(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    name: Option<String>,
    command: Option<String>,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    template: Option<PanelTemplateRef>,
) -> Result<Panel> {
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

    let has_custom_name = name.is_some();
    let title = name.unwrap_or_else(|| {
        command
            .as_deref()
            .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_string_lossy().to_string()))
            .unwrap_or_else(|| "Markdown".to_string())
    });

    tracing::info!("created editor panel '{}' (id={})", title, id.0);

    Ok(Panel {
        id,
        local_id,
        title,
        kind: PanelKind::Editor,
        resume: PanelResume::Fresh,
        layout: PanelLayout {
            position: position.unwrap_or_default(),
            size: size.unwrap_or(DEFAULT_PANEL_SIZE),
        },
        workspace_id,
        content: PanelContent::Editor(editor),
        session_binding: None,
        template,
        launched_at_millis: current_unix_millis(),
        has_custom_name,
        had_recent_output: false,
        launch_command: command,
        launch_args: Vec::new(),
        launch_cwd: None,
    })
}

#[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
fn spawn_git_changes(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    name: Option<String>,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    template: Option<PanelTemplateRef>,
    cwd: Option<PathBuf>,
) -> Result<Panel> {
    let has_custom_name = name.is_some();
    let title = name.unwrap_or_else(|| "Git Changes".to_string());
    tracing::info!("created git changes panel '{}' (id={})", title, id.0);

    Ok(Panel {
        id,
        local_id,
        title,
        kind: PanelKind::GitChanges,
        resume: PanelResume::Fresh,
        layout: PanelLayout {
            position: position.unwrap_or_default(),
            size: size.unwrap_or(DEFAULT_PANEL_SIZE),
        },
        workspace_id,
        content: PanelContent::GitChanges(crate::git_changes::GitChangesViewer::new()),
        session_binding: None,
        template,
        launched_at_millis: current_unix_millis(),
        has_custom_name,
        had_recent_output: false,
        launch_command: None,
        launch_args: Vec::new(),
        launch_cwd: cwd,
    })
}

#[allow(clippy::unnecessary_wraps)]
fn spawn_usage(
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    name: Option<String>,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    template: Option<PanelTemplateRef>,
) -> Result<Panel> {
    let has_custom_name = name.is_some();
    let title = name.unwrap_or_else(|| "Usage".to_string());
    tracing::info!("created usage panel '{}' (id={})", title, id.0);

    Ok(Panel {
        id,
        local_id,
        title,
        kind: PanelKind::Usage,
        resume: PanelResume::Fresh,
        layout: PanelLayout {
            position: position.unwrap_or_default(),
            size: size.unwrap_or(DEFAULT_PANEL_SIZE),
        },
        workspace_id,
        content: PanelContent::Usage(UsageDashboard::new()),
        session_binding: None,
        template,
        launched_at_millis: current_unix_millis(),
        has_custom_name,
        had_recent_output: false,
        launch_command: None,
        launch_args: Vec::new(),
        launch_cwd: None,
    })
}

pub(super) fn resolve_launch_command(
    command: Option<String>,
    args: Vec<String>,
    kind: PanelKind,
    resume: &PanelResume,
    session_binding: Option<&AgentSessionBinding>,
    should_resume_binding: bool,
) -> (String, Vec<String>) {
    match kind {
        PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => (String::new(), Vec::new()),
        PanelKind::Shell => (command.unwrap_or_else(default_shell), args),
        PanelKind::Command => {
            if let Some(program) = command {
                (program, args)
            } else {
                (default_shell(), args)
            }
        }
        PanelKind::Codex => {
            let program = command.unwrap_or_else(|| "codex".to_string());
            let mut launch_args = args;
            if should_resume_binding {
                if let Some(binding) = session_binding {
                    launch_args.extend(["resume".to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend(["resume".to_string(), session_id.clone()]);
            }
            wrap_in_login_shell(program, launch_args)
        }
        PanelKind::Claude => {
            let program = command.unwrap_or_else(|| "claude".to_string());
            let mut launch_args = Vec::new();
            if let Some(plugin_path) = horizon_claude_plugin_dir() {
                launch_args.extend(["--plugin-dir".to_string(), plugin_path]);
            }
            if let Some(binding) = session_binding {
                if should_resume_binding {
                    launch_args.extend(["--resume".to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend(["--resume".to_string(), session_id.clone()]);
            } else {
                launch_args.extend(["--session-id".to_string(), Uuid::new_v4().to_string()]);
            }
            launch_args.extend(args);
            wrap_in_login_shell(program, launch_args)
        }
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
) -> (Option<PanelTranscript>, Vec<u8>) {
    let mut transcript = PanelTranscript::for_panel(kind, transcript_root, local_id);
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

    (transcript, replay_bytes)
}

fn resolve_session_binding(
    kind: PanelKind,
    resume: &PanelResume,
    mut session_binding: Option<AgentSessionBinding>,
    cwd: Option<&str>,
    label: Option<&str>,
) -> (Option<AgentSessionBinding>, bool) {
    let had_existing_session_binding = session_binding.is_some();
    if session_binding.is_none() {
        // Claude fresh launches intentionally start without a synthetic
        // binding. The CLI only writes a real session record after the
        // first user message, so preassigning an ID would not match any
        // on-disk session state.
        session_binding = match (resume, kind) {
            (PanelResume::Session { session_id }, PanelKind::Codex | PanelKind::Claude) => {
                Some(AgentSessionBinding::new(
                    kind,
                    session_id.clone(),
                    cwd.map(str::to_string),
                    label.map(str::to_string),
                    None,
                ))
            }
            _ => None,
        };
    }

    let should_resume_binding = match kind {
        PanelKind::Claude => {
            session_binding.is_some()
                && (had_existing_session_binding || matches!(resume, PanelResume::Last | PanelResume::Session { .. }))
        }
        PanelKind::Codex
        | PanelKind::Shell
        | PanelKind::Command
        | PanelKind::Editor
        | PanelKind::GitChanges
        | PanelKind::Usage => session_binding.is_some() || matches!(resume, PanelResume::Session { .. }),
    };

    (session_binding, should_resume_binding)
}

fn wrap_in_login_shell(program: String, args: Vec<String>) -> (String, Vec<String>) {
    let shell = default_shell();
    let mut command = vec![program];
    command.extend(args);
    let joined = command
        .iter()
        .map(|argument| shell_escape(argument))
        .collect::<Vec<_>>()
        .join(" ");
    (shell, vec!["-ic".to_string(), joined])
}

fn shell_escape(argument: &str) -> String {
    if argument.is_empty()
        || argument.contains(|character: char| {
            character.is_whitespace() || character == '\'' || character == '"' || character == '\\' || character == '$'
        })
    {
        format!("'{}'", argument.replace('\'', "'\\''"))
    } else {
        argument.to_string()
    }
}

pub(super) const fn platform_default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| platform_default_shell().to_string())
}

pub(super) fn agent_env(kind: PanelKind) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if kind.is_agent() {
        env.insert("HORIZON".to_string(), "1".to_string());
    }
    env
}

fn horizon_claude_plugin_dir() -> Option<String> {
    let path = HorizonHome::resolve().claude_plugin_dir();
    path.is_dir().then(|| path.display().to_string())
}

pub(super) fn scrollback_limit_for_kind(kind: PanelKind) -> usize {
    match kind {
        PanelKind::Codex | PanelKind::Claude => AGENT_PANEL_SCROLLBACK_LIMIT,
        PanelKind::Shell | PanelKind::Command => DEFAULT_PANEL_SCROLLBACK_LIMIT,
        PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => 0,
    }
}

pub(super) fn kitty_keyboard_for_kind(kind: PanelKind) -> bool {
    !matches!(kind, PanelKind::Codex)
}
