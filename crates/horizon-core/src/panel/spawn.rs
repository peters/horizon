mod terminal;

pub(super) use terminal::restore_failure_panel;
use terminal::spawn_terminal;
#[cfg(test)]
use terminal::{disconnected_snapshot_launch_command, prepare_transcript_restore};

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::agents::AgentStatus;
use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::git_changes::DiffViewer;
use crate::horizon_home::HorizonHome;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, claude_session_transcript_exists, new_local_id};
use crate::ssh::SshConnection;
use crate::transcript::PanelTranscript;
use crate::usage_dashboard::UsageDashboard;
use crate::workspace::WorkspaceId;
use crate::{AgentIntegrationKind, AgentResumeMode, agent_definition};

use super::{
    AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind,
    PanelLayout, PanelOptions, PanelResume,
};

struct StaticPanelSeed {
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    name: Option<String>,
    name_is_custom: Option<bool>,
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

struct ResolvedTerminalLaunch {
    session_binding: Option<AgentSessionBinding>,
    program: String,
    launch_args: Vec<String>,
}

/// Session-related inputs for building an agent launch command.
#[derive(Clone, Copy)]
pub(super) struct AgentLaunchContext<'a> {
    pub(super) resume: &'a PanelResume,
    pub(super) session_binding: Option<&'a AgentSessionBinding>,
    pub(super) should_resume_binding: bool,
    /// True when reconnecting an existing panel (restore or restart) rather
    /// than launching a newly added one; continue-style agents only pass
    /// their continue flag when reconnecting.
    pub(super) is_restore: bool,
}

impl StaticPanelSeed {
    fn from_options(id: PanelId, workspace_id: WorkspaceId, local_id: String, opts: &mut PanelOptions) -> Self {
        Self {
            id,
            workspace_id,
            local_id,
            name: opts.name.take(),
            name_is_custom: opts.name_is_custom,
            position: opts.position,
            size: opts.size,
            template: opts.template.take(),
        }
    }

    fn take_title(&mut self, fallback: impl FnOnce() -> String) -> (String, bool) {
        let has_custom_name = self.name_is_custom.unwrap_or_else(|| self.name.is_some());
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
            remote_workspace: None,
            title,
            terminal_title: String::new(),
            kind,
            resume: PanelResume::Fresh,
            layout: PanelLayout {
                position: self.position.unwrap_or_default(),
                size: self.size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            visible: true,
            workspace_id: self.workspace_id,
            content,
            session_binding: None,
            template: self.template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            had_recent_output: false,
            agent_status: AgentStatus::default(),
            last_output_at_millis: None,
            launch_command,
            launch_args: Vec::new(),
            launch_cwd,
            ssh_connection: None,
            ssh_status: None,
        }
    }
}

pub(super) fn spawn_panel(id: PanelId, workspace_id: WorkspaceId, mut opts: PanelOptions) -> Result<Panel> {
    if opts.remote_workspace.is_some() {
        return restore_failure_panel(id, workspace_id, opts, "Remote connection pending");
    }
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);

    match opts.kind {
        PanelKind::Editor => {
            let command = opts.command.take();
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            spawn_editor(seed, command)
        }
        PanelKind::GitChanges => {
            let cwd = opts.cwd.take();
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            Ok(spawn_git_changes(seed, cwd))
        }
        PanelKind::Usage => {
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            Ok(spawn_usage(seed))
        }
        PanelKind::Browser => {
            let command = opts.command.take();
            let browser_config = opts.browser_config.take();
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            spawn_browser(seed, command, browser_config)
        }
        _ => spawn_terminal(id, workspace_id, local_id, opts),
    }
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
    saved_cwd: Option<&PathBuf>,
    transcript: Option<&PanelTranscript>,
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
            is_restore,
        },
    );

    let launch_trace = TerminalLaunchTrace {
        kind,
        resume,
        session_binding: session_binding.as_ref(),
        should_resume_binding,
        cwd: saved_cwd_string.as_deref(),
        cmd: format!("{program} {}", launch_args.join(" ")),
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

/// Spawn a browser panel. The generic `command` field carries the optional
/// initial URL (same convention as the editor's file path); `browser_config`
/// is the active `browser` config section (honors `--config`).
fn spawn_browser(
    mut seed: StaticPanelSeed,
    initial_url: Option<String>,
    browser_config: Option<crate::browser::BrowserConfig>,
) -> Result<Panel> {
    let initial_url = initial_url.filter(|url| !url.trim().is_empty());
    let (title, has_custom_name) = seed.take_title(|| {
        initial_url
            .as_deref()
            .map_or_else(|| "Browser".to_string(), crate::browser::panel_title_for_url)
    });
    let browser = crate::browser::BrowserPanelState::start(
        seed.local_id.clone(),
        &browser_config.unwrap_or_default(),
        initial_url.clone(),
    )?;
    tracing::info!("created browser panel '{}' (id={})", title, seed.id.0);

    Ok(seed.into_panel(
        title,
        PanelKind::Browser,
        PanelContent::Browser(Box::new(browser)),
        initial_url,
        None,
        has_custom_name,
    ))
}

pub(super) fn resolve_launch_command(
    command: Option<String>,
    args: Vec<String>,
    ssh_connection: Option<SshConnection>,
    kind: PanelKind,
    launch: AgentLaunchContext<'_>,
) -> (String, Vec<String>) {
    match kind {
        PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage | PanelKind::Browser => {
            (String::new(), Vec::new())
        }
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
        | PanelKind::Pi
        | PanelKind::Grok => resolve_agent_launch_command(command, args, kind, launch),
    }
}

fn resolve_agent_launch_command(
    command: Option<String>,
    args: Vec<String>,
    kind: PanelKind,
    launch: AgentLaunchContext<'_>,
) -> (String, Vec<String>) {
    let Some(definition) = agent_definition(kind) else {
        unreachable!("agent launch requested for non-agent panel: {kind:?}");
    };
    let uses_default_command = command.is_none();
    let program = command.unwrap_or_else(|| definition.default_command.to_string());
    let mut launch_args = match definition.integration {
        AgentIntegrationKind::CodexMcp if uses_default_command => horizon_codex_mcp_args(),
        AgentIntegrationKind::None | AgentIntegrationKind::CodexMcp => Vec::new(),
        AgentIntegrationKind::ClaudePluginDir => horizon_claude_plugin_args(),
    };
    match definition.resume_mode {
        AgentResumeMode::ExactSubcommand { subcommand } => {
            launch_args.extend(args);
            if launch.should_resume_binding {
                if let Some(binding) = launch.session_binding {
                    launch_args.extend([subcommand.to_string(), binding.session_id.clone()]);
                }
            } else if let PanelResume::Session { session_id } = launch.resume {
                launch_args.extend([subcommand.to_string(), session_id.clone()]);
            }
        }
        AgentResumeMode::ExactFlag {
            flag,
            fresh_session_flag,
        } => {
            if launch.should_resume_binding {
                if let Some(binding) = launch.session_binding {
                    launch_args.extend([flag.to_string(), binding.session_id.clone()]);
                }
            } else if let (Some(fresh_session_flag), Some(binding)) = (fresh_session_flag, launch.session_binding) {
                // Fresh launch under the panel's pre-assigned session id so
                // the binding matches the session the CLI will create.
                launch_args.extend([fresh_session_flag.to_string(), binding.session_id.clone()]);
            } else if let PanelResume::Session { session_id } = launch.resume {
                launch_args.extend([flag.to_string(), session_id.clone()]);
            } else if let Some(fresh_session_flag) = fresh_session_flag {
                launch_args.extend([fresh_session_flag.to_string(), Uuid::new_v4().to_string()]);
            }
            launch_args.extend(args);
        }
        AgentResumeMode::ContinueFlag { flag } => {
            launch_args.extend(args);
            // Continue-style agents have no per-session ids, so `resume:
            // last` maps to the continue flag, and only when reconnecting an
            // existing panel; a newly added panel always starts fresh.
            if launch.is_restore && matches!(launch.resume, PanelResume::Last) {
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

pub(super) fn agent_env(kind: PanelKind, local_id: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    if kind.is_agent() {
        env.insert("HORIZON".to_string(), "1".to_string());
        env.insert("HORIZON_BROWSER_ACTOR".to_string(), browser_actor(local_id));
        env.insert(
            crate::browser::manifest::HOST_INSTANCE_ENV.to_string(),
            crate::browser::manifest::host_instance().to_string(),
        );
    }
    if kind == PanelKind::Claude {
        // Keep the conversation in Horizon's terminal history so its scrollbar
        // and history meter remain usable instead of hiding it in a fullscreen buffer.
        env.insert("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN".to_string(), "1".to_string());
    }
    env
}

/// Stable private control identity injected into one Horizon agent panel.
#[must_use]
pub fn browser_actor(local_id: &str) -> String {
    if !local_id.is_empty() && local_id.len() <= 120 && !local_id.chars().any(char::is_control) {
        return format!("horizon:{local_id}");
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    local_id.hash(&mut hasher);
    format!("horizon:{:016x}", hasher.finish())
}

fn horizon_codex_mcp_args() -> Vec<String> {
    let Some(command) = crate::browser_mcp_executable()
        .and_then(|path| path.into_os_string().into_string().ok())
        .and_then(|path| serde_json::to_string(&path).ok())
    else {
        tracing::warn!("could not resolve Horizon executable for browser MCP integration");
        return Vec::new();
    };
    vec![
        "-c".to_string(),
        format!("mcp_servers.horizon-browser.command={command}"),
        "-c".to_string(),
        "mcp_servers.horizon-browser.args=[\"--browser-mcp\"]".to_string(),
        "-c".to_string(),
        "mcp_servers.horizon-browser.env_vars=[\"HORIZON_BROWSER_ACTOR\",\"HORIZON_BROWSER_HOST_INSTANCE\"]"
            .to_string(),
        "-c".to_string(),
        "mcp_servers.horizon-browser.default_tools_approval_mode=\"approve\"".to_string(),
    ]
}

fn horizon_claude_plugin_args() -> Vec<String> {
    let path = HorizonHome::resolve().claude_plugin_dir_for_host(crate::browser::manifest::host_instance());
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
            PanelKind::Editor | PanelKind::GitChanges | PanelKind::Usage | PanelKind::Browser => 0,
            PanelKind::Codex
            | PanelKind::Claude
            | PanelKind::OpenCode
            | PanelKind::Gemini
            | PanelKind::KiloCode
            | PanelKind::Pi
            | PanelKind::Grok => unreachable!(),
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
    fn restored_generated_name_stays_non_custom() {
        let mut options = PanelOptions {
            name: Some("127.0.0.1".to_string()),
            name_is_custom: Some(false),
            ..PanelOptions::default()
        };
        let mut seed =
            StaticPanelSeed::from_options(PanelId(1), WorkspaceId(1), "browser-panel".to_string(), &mut options);

        let (title, has_custom_name) = seed.take_title(|| "Browser".to_string());

        assert_eq!(title, "127.0.0.1");
        assert!(!has_custom_name);
    }

    #[test]
    fn legacy_supplied_name_remains_custom() {
        let mut options = PanelOptions {
            name: Some("Pinned name".to_string()),
            ..PanelOptions::default()
        };
        let mut seed =
            StaticPanelSeed::from_options(PanelId(1), WorkspaceId(1), "browser-panel".to_string(), &mut options);

        let (_, has_custom_name) = seed.take_title(|| "Browser".to_string());

        assert!(has_custom_name);
    }

    #[test]
    fn agent_environment_exposes_a_stable_browser_actor() {
        let env = agent_env(PanelKind::Codex, "panel-42");
        assert_eq!(env.get("HORIZON").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("HORIZON_BROWSER_ACTOR").map(String::as_str),
            Some("horizon:panel-42")
        );
        assert_eq!(
            env.get(crate::browser::manifest::HOST_INSTANCE_ENV).map(String::as_str),
            Some(crate::browser::manifest::host_instance())
        );
        assert!(agent_env(PanelKind::Shell, "panel-42").is_empty());
        assert_eq!(browser_actor(&"x".repeat(512)).len(), 24);
    }

    #[test]
    fn default_codex_launch_registers_only_the_mcp_browser_contract() {
        let (_, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Codex,
            fresh_launch_context(&PanelResume::Fresh),
        );
        let command = args.join(" ");
        assert!(command.contains("mcp_servers.horizon-browser.command="));
        assert!(command.contains("mcp_servers.horizon-browser.args="));
        assert!(command.contains(
            "mcp_servers.horizon-browser.env_vars=[\"HORIZON_BROWSER_ACTOR\",\"HORIZON_BROWSER_HOST_INSTANCE\"]"
        ));
        assert!(command.contains("mcp_servers.horizon-browser.default_tools_approval_mode=\"approve\""));
        assert!(command.contains("--browser-mcp"));
        assert!(!command.contains("browser-cli"));
        #[cfg(target_os = "linux")]
        assert!(command.contains(&format!("/proc/{}/exe", std::process::id())));
    }

    #[test]
    fn custom_codex_launch_does_not_inject_the_horizon_mcp_registration() {
        let (_, args) = resolve_launch_command(
            Some("/opt/custom-codex".to_string()),
            vec!["--custom-flag".to_string()],
            None,
            PanelKind::Codex,
            fresh_launch_context(&PanelResume::Fresh),
        );
        let command = args.join(" ");
        assert!(command.contains("/opt/custom-codex --custom-flag"));
        assert!(!command.contains("mcp_servers.horizon-browser"));
        assert!(!command.contains("--browser-mcp"));
    }

    #[test]
    fn claude_uses_horizon_native_scrollback() {
        let claude_env = agent_env(PanelKind::Claude, "claude-panel");

        assert_eq!(
            claude_env
                .get("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN")
                .map(String::as_str),
            Some("1")
        );
        assert!(!agent_env(PanelKind::Codex, "codex-panel").contains_key("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"));
        assert!(!agent_env(PanelKind::Shell, "shell-panel").contains_key("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"));
    }

    #[test]
    fn shell_launch_args_adds_login_flag_when_requested() {
        assert_eq!(shell_launch_args(Vec::new(), true), vec!["-l".to_string()]);
    }

    #[test]
    fn disconnected_snapshot_launch_command_exits_without_reconnecting() {
        let (program, args) = disconnected_snapshot_launch_command();

        if cfg!(windows) {
            assert_eq!(program, "cmd.exe");
            assert_eq!(args, vec!["/C".to_string(), "exit".to_string()]);
        } else {
            assert_eq!(program, default_shell());
            assert_eq!(args, vec!["-c".to_string(), "exit".to_string()]);
        }
    }

    #[test]
    fn prepare_transcript_restore_treats_empty_root_as_fresh_state() {
        let transcript_root = tempfile::tempdir().expect("tempdir");

        let (_, replay_bytes, had_persisted_state) = prepare_transcript_restore(
            PanelId(1),
            PanelKind::Ssh,
            Some(transcript_root.path().to_path_buf()),
            "ssh-panel",
        );

        assert!(replay_bytes.is_empty());
        assert!(!had_persisted_state);
    }

    #[test]
    fn prepare_transcript_restore_detects_empty_persisted_transcript() {
        let transcript_root = tempfile::tempdir().expect("tempdir");
        std::fs::write(transcript_root.path().join("ssh-panel.bin"), b"").expect("write transcript");

        let (_, replay_bytes, had_persisted_state) = prepare_transcript_restore(
            PanelId(1),
            PanelKind::Ssh,
            Some(transcript_root.path().to_path_buf()),
            "ssh-panel",
        );

        assert!(replay_bytes.is_empty());
        assert!(had_persisted_state);
    }

    fn fresh_launch_context(resume: &PanelResume) -> AgentLaunchContext<'_> {
        AgentLaunchContext {
            resume,
            session_binding: None,
            should_resume_binding: false,
            is_restore: false,
        }
    }

    #[test]
    fn resolve_launch_command_preserves_custom_shell_without_args() {
        let (program, args) = resolve_launch_command(
            Some("/usr/local/bin/custom-shell".to_string()),
            Vec::new(),
            None,
            PanelKind::Shell,
            fresh_launch_context(&PanelResume::Fresh),
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
            fresh_launch_context(&PanelResume::Fresh),
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
            fresh_launch_context(&PanelResume::Fresh),
        );

        assert_eq!(program, "ssh");
        assert_eq!(
            args,
            vec![
                "-p".to_string(),
                "2222".to_string(),
                "-o".to_string(),
                "ServerAliveInterval=15".to_string(),
                "-o".to_string(),
                "ServerAliveCountMax=1".to_string(),
                "deploy@prod-api".to_string(),
            ]
        );
    }

    #[test]
    fn claude_fresh_launch_preassigns_session_binding() {
        let (binding, should_resume) = resolve_session_binding(
            PanelKind::Claude,
            &PanelResume::Fresh,
            None,
            Some("/repo"),
            None,
            |_| false,
        );

        let binding = binding.expect("fresh Claude panels are bound to their session at launch");
        assert!(!should_resume);
        assert_eq!(binding.kind, PanelKind::Claude);
        assert_eq!(binding.cwd.as_deref(), Some("/repo"));
        assert!(!binding.session_id.is_empty());
        assert!(binding.updated_at.is_some());
    }

    #[test]
    fn non_claude_fresh_launch_stays_unbound() {
        let (binding, should_resume) =
            resolve_session_binding(PanelKind::Codex, &PanelResume::Fresh, None, Some("/repo"), None, |_| {
                false
            });

        assert!(binding.is_none());
        assert!(!should_resume);
    }

    #[test]
    fn claude_binding_resumes_only_when_transcript_exists() {
        let binding = AgentSessionBinding::new(PanelKind::Claude, "session-1".to_string(), None, None, None);

        let (_, resume_with_missing_transcript) = resolve_session_binding(
            PanelKind::Claude,
            &PanelResume::Fresh,
            Some(binding.clone()),
            None,
            None,
            |_| false,
        );
        let (_, resume_with_existing_transcript) = resolve_session_binding(
            PanelKind::Claude,
            &PanelResume::Fresh,
            Some(binding),
            None,
            None,
            |_| true,
        );

        assert!(!resume_with_missing_transcript);
        assert!(resume_with_existing_transcript);
    }

    #[test]
    fn claude_fresh_launch_command_uses_preassigned_session_id() {
        let binding = AgentSessionBinding::new(
            PanelKind::Claude,
            "11111111-2222-3333-4444-555555555555".to_string(),
            None,
            None,
            None,
        );

        let (_, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Claude,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: Some(&binding),
                should_resume_binding: false,
                is_restore: false,
            },
        );

        let joined = args.join(" ");
        assert!(
            joined.contains("--session-id 11111111-2222-3333-4444-555555555555"),
            "{joined}"
        );
        assert!(!joined.contains("--resume"), "{joined}");
    }
}
