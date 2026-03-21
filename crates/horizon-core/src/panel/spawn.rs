use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::git_changes::DiffViewer;
use crate::horizon_home::HorizonHome;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, new_local_id};
use crate::ssh::{SshConnection, SshConnectionStatus};
use crate::terminal::{Terminal, TerminalSpawnOptions};
use crate::transcript::PanelTranscript;
use crate::usage_dashboard::UsageDashboard;
use crate::workspace::WorkspaceId;
use crate::{AgentIntegrationKind, AgentResumeMode, agent_definition};

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
    cwd: Option<&'a str>,
    cmd: String,
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
            launch_command,
            launch_args: Vec::new(),
            launch_cwd,
            ssh_connection: None,
            ssh_status: None,
        }
    }
}

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
        ..
    } = opts;

    let (transcript, replay_bytes) = prepare_transcript_restore(id, kind, transcript_root, &local_id);
    let saved_command = command.clone();
    let saved_args = args.clone();
    let saved_cwd = cwd.clone();
    let saved_ssh_connection = ssh_connection.clone();
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
        ssh_connection.clone(),
        kind,
        &resume,
        session_binding.as_ref(),
        should_resume_binding,
    );

    let launch_trace = TerminalLaunchTrace {
        kind,
        resume: &resume,
        session_binding: session_binding.as_ref(),
        should_resume_binding,
        cwd: saved_cwd_string.as_deref(),
        cmd: format!("{program} {}", launch_args.join(" ")),
    };
    log_terminal_launch(id, &launch_trace);

    let (program, launch_args) = if let Some(transcript) = transcript.as_ref() {
        transcript.wrap_launch_command(program, launch_args)
    } else {
        (program, launch_args)
    };
    let has_custom_name = name.is_some();
    let title = name.unwrap_or_else(|| default_terminal_title(id, saved_ssh_connection.as_ref()));
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
        terminal_title: String::new(),
        launch_command: saved_command,
        launch_args: saved_args,
        launch_cwd: saved_cwd,
        ssh_connection: saved_ssh_connection,
        ssh_status: if kind == PanelKind::Ssh {
            Some(SshConnectionStatus::Connecting)
        } else {
            None
        },
    })
}

fn default_terminal_title(id: PanelId, ssh_connection: Option<&SshConnection>) -> String {
    ssh_connection.map_or_else(
        || format!("Terminal {}", id.0),
        |connection| format!("SSH: {}", connection.display_label()),
    )
}

fn log_terminal_launch(id: PanelId, trace: &TerminalLaunchTrace<'_>) {
    if !trace.kind.is_agent() {
        return;
    }

    tracing::info!(
        panel_id = id.0,
        kind = ?trace.kind,
        resume = ?trace.resume,
        session_id = trace.session_binding.map(|binding| binding.session_id.as_str()),
        should_resume = trace.should_resume_binding,
        cwd = trace.cwd,
        cmd = %trace.cmd,
        "launching agent panel"
    );
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
    resume: &PanelResume,
    session_binding: Option<&AgentSessionBinding>,
    should_resume_binding: bool,
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
        PanelKind::Codex | PanelKind::Claude | PanelKind::OpenCode | PanelKind::Gemini | PanelKind::KiloCode => {
            resolve_agent_launch_command(command, args, kind, resume, session_binding, should_resume_binding)
        }
    }
}

fn resolve_agent_launch_command(
    command: Option<String>,
    args: Vec<String>,
    kind: PanelKind,
    resume: &PanelResume,
    session_binding: Option<&AgentSessionBinding>,
    should_resume_binding: bool,
) -> (String, Vec<String>) {
    let Some(definition) = agent_definition(kind) else {
        unreachable!("agent launch requested for non-agent panel: {kind:?}");
    };
    let program = command.unwrap_or_else(|| definition.default_command.to_string());
    let mut launch_args = match definition.integration {
        AgentIntegrationKind::None => Vec::new(),
        AgentIntegrationKind::ClaudePluginDir => horizon_claude_plugin_args(),
    };

    match definition.resume_mode {
        AgentResumeMode::ExactSubcommand { subcommand } => {
            launch_args.extend(args);
            if should_resume_binding {
                if let Some(binding) = session_binding {
                    launch_args.extend([subcommand.to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend([subcommand.to_string(), session_id.clone()]);
            }
        }
        AgentResumeMode::ExactFlag {
            flag,
            fresh_session_flag,
        } => {
            if should_resume_binding {
                if let Some(binding) = session_binding {
                    launch_args.extend([flag.to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = resume {
                launch_args.extend([flag.to_string(), session_id.clone()]);
            } else if let Some(fresh_session_flag) = fresh_session_flag {
                launch_args.extend([fresh_session_flag.to_string(), Uuid::new_v4().to_string()]);
            }
            launch_args.extend(args);
        }
        AgentResumeMode::ContinueFlag { flag } => {
            launch_args.extend(args);
            if matches!(resume, PanelResume::Last) {
                launch_args.push(flag.to_string());
            }
        }
        AgentResumeMode::None => launch_args.extend(args),
    }

    wrap_in_login_shell(program, launch_args)
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
            (PanelResume::Session { session_id }, kind) if kind.supports_session_binding() => {
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

    let should_resume_binding = if kind == PanelKind::Claude {
        session_binding.is_some()
            && (had_existing_session_binding || matches!(resume, PanelResume::Last | PanelResume::Session { .. }))
    } else {
        session_binding.is_some() || matches!(resume, PanelResume::Session { .. })
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

fn horizon_claude_plugin_args() -> Vec<String> {
    let path = HorizonHome::resolve().claude_plugin_dir();
    if path.is_dir() {
        vec!["--plugin-dir".to_string(), path.display().to_string()]
    } else {
        Vec::new()
    }
}

pub(super) fn scrollback_limit_for_kind(kind: PanelKind) -> usize {
    if kind.is_agent() {
        AGENT_PANEL_SCROLLBACK_LIMIT
    } else {
        match kind {
            PanelKind::Shell | PanelKind::Ssh | PanelKind::Command => DEFAULT_PANEL_SCROLLBACK_LIMIT,
            PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage => 0,
            PanelKind::Codex | PanelKind::Claude | PanelKind::OpenCode | PanelKind::Gemini | PanelKind::KiloCode => {
                unreachable!()
            }
        }
    }
}

pub(super) fn kitty_keyboard_for_kind(kind: PanelKind) -> bool {
    agent_definition(kind).is_none_or(|definition| definition.kitty_keyboard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_launch_args_adds_login_flag_when_requested() {
        assert_eq!(shell_launch_args(Vec::new(), true), vec!["-l".to_string()]);
    }

    #[test]
    fn resolve_launch_command_preserves_custom_shell_without_args() {
        let (program, args) = resolve_launch_command(
            Some("/usr/local/bin/custom-shell".to_string()),
            Vec::new(),
            None,
            PanelKind::Shell,
            &PanelResume::Fresh,
            None,
            false,
        );

        assert_eq!(program, "/usr/local/bin/custom-shell");
        assert!(args.is_empty());
    }

    #[test]
    fn resolve_launch_command_adds_login_flag_only_for_default_shell() {
        let (program, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Shell,
            &PanelResume::Fresh,
            None,
            false,
        );

        assert_eq!(program, default_shell());
        if PLATFORM_USES_LOGIN_SHELL {
            assert_eq!(args, vec!["-l".to_string()]);
        } else {
            assert!(args.is_empty());
        }
    }

    #[test]
    fn resolve_launch_command_prefers_structured_ssh_connection() {
        let connection = SshConnection {
            host: "prod-api".to_string(),
            user: Some("deploy".to_string()),
            port: Some(2222),
            ..SshConnection::default()
        };

        let (program, args) = resolve_launch_command(
            Some("custom-ignored".to_string()),
            vec!["--ignored".to_string()],
            Some(connection),
            PanelKind::Ssh,
            &PanelResume::Fresh,
            None,
            false,
        );

        assert_eq!(program, "ssh");
        assert_eq!(
            args,
            vec!["-p".to_string(), "2222".to_string(), "deploy@prod-api".to_string(),]
        );
    }
}
