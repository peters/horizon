mod device;
mod terminal;

use terminal::spawn_terminal;
pub(super) use terminal::{Placeholder, placeholder_lines, placeholder_panel, placeholder_terminal};
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
            work_resume: crate::agent_work::ResumePolicy::default(),
            layout: PanelLayout {
                position: self.position.unwrap_or_default(),
                size: self.size.unwrap_or(DEFAULT_PANEL_SIZE),
            },
            visible: true,
            workspace_id: self.workspace_id,
            content,
            transient: false,
            disconnected_browser_profile: None,
            disconnected_device_identity: None,
            cloud_wait: None,
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
        return placeholder_panel(
            id,
            workspace_id,
            opts,
            Placeholder::RestoreFailure("Remote development has been removed; this saved view is read-only"),
        );
    }
    let deleting_binding = opts
        .session_binding
        .as_ref()
        .is_some_and(|binding| crate::runtime_state::saved_session_deletion_pending(opts.kind, &binding.session_id));
    let deleting_resume = matches!(&opts.resume, PanelResume::Session { session_id } if crate::runtime_state::saved_session_deletion_pending(opts.kind, session_id));
    if deleting_binding || deleting_resume {
        return Err(crate::error::Error::State(
            "This saved conversation is being deleted".into(),
        ));
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
        PanelKind::SetupBoard => {
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            Ok(spawn_setup_board(seed))
        }
        PanelKind::Device => {
            let command = opts.command.take();
            let is_restore = opts.is_restore;
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            device::spawn_device(
                seed,
                command.as_deref(),
                is_restore,
                opts.device_identity.take(),
                opts.device_input,
                opts.ssh_connection.take(),
            )
        }
        PanelKind::Browser => {
            let command = opts.command.take();
            let browser_config = opts.browser_config.take();
            let follows_browser_host = opts.follows_browser_host;
            #[cfg(feature = "cloud-workspaces")]
            let cloud_target = opts.remote_target.clone();
            let remote = match (opts.remote_session.take(), opts.remote_target.take()) {
                (Some(request), _) => BrowserSpawnMode::Remote(Box::new(request)),
                (None, Some(target)) => BrowserSpawnMode::RestoredRemote(target),
                (None, None) => BrowserSpawnMode::Local(opts.browser_session_id.take()),
            };
            #[cfg(feature = "cloud-workspaces")]
            let remote = opts
                .cloud_connection
                .take()
                .map_or(remote, |connection| BrowserSpawnMode::Cloud(connection, cloud_target));
            let remote = browser_spawn_mode(remote, crate::browser_host(), follows_browser_host)?;
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            spawn_browser(seed, command, browser_config, remote)
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

fn spawn_setup_board(mut seed: StaticPanelSeed) -> Panel {
    let (title, has_custom_name) = seed.take_title(|| "Setup board".to_string());
    tracing::info!("created setup board panel '{}' (id={})", title, seed.id.0);
    seed.into_panel(
        title,
        PanelKind::SetupBoard,
        PanelContent::SetupBoard(crate::SetupBoard::default()),
        None,
        None,
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

/// Where a browser panel's session runs.
enum BrowserSpawnMode {
    Local(Option<String>),
    /// A prepared remote request: allocate at the grid instead of launching.
    Remote(Box<horizon_browser::RemoteSessionRequest>),
    /// A remote panel from a previous run; it comes back stopped.
    RestoredRemote(String),
    /// The browser host of another machine that the settings name.
    RemoteHost(crate::browser::RemoteHostBackend),
    #[cfg(feature = "cloud-workspaces")]
    Cloud(crate::cloud_runtime::ssh::Connection, Option<String>),
}

/// The backend of a new browser panel: the browser host that the settings name,
/// or the browser of this computer.
///
/// Only a panel that a person creates from the preset picker follows the entry,
/// which its `PanelOptions` says. A page that an agent asks for stays on the
/// browser of this computer, because the entry of that agent reaches the host
/// over the bridge and a signed-in profile of the person takes no page that an
/// agent asked for. A saved panel, and a page of a local profile that a panel
/// duplicates, keep the backend that their caller chose.
///
/// # Errors
/// Returns the reason that `RemoteHostBackend::new` gives for a target that `ssh`
/// would take as an option or split, so no such value reaches a channel.
fn browser_spawn_mode(
    mode: BrowserSpawnMode,
    browser_host: Option<&crate::BrowserHostConfig>,
    follows_browser_host: bool,
) -> Result<BrowserSpawnMode> {
    if !follows_browser_host {
        return Ok(mode);
    }
    let Some(browser_host) = browser_host.filter(|browser_host| browser_host.names_host()) else {
        return Ok(mode);
    };
    if !matches!(mode, BrowserSpawnMode::Local(None)) {
        return Ok(mode);
    }
    let backend = crate::browser::RemoteHostBackend::new(
        browser_host.ssh_target().unwrap_or_default(),
        browser_host.profile().map(str::to_string),
    )?;
    Ok(BrowserSpawnMode::RemoteHost(backend))
}

/// Spawn a browser panel. The generic `command` field carries the optional
/// initial URL (same convention as the editor's file path); `browser_config`
/// is the active `browser` config section (honors `--config`).
fn spawn_browser(
    mut seed: StaticPanelSeed,
    initial_url: Option<String>,
    browser_config: Option<crate::browser::BrowserConfig>,
    mode: BrowserSpawnMode,
) -> Result<Panel> {
    let initial_url = initial_url.filter(|url| !url.trim().is_empty());
    let (title, has_custom_name) = seed.take_title(|| {
        initial_url
            .as_deref()
            .map_or_else(|| "Browser".to_string(), crate::browser::panel_title_for_url)
    });
    let browser_config = browser_config.unwrap_or_default();
    let browser = match mode {
        #[cfg(feature = "cloud-workspaces")]
        BrowserSpawnMode::Cloud(connection, target) => crate::browser::BrowserPanelState::start_cloud(
            seed.local_id.clone(),
            connection,
            target,
            initial_url.clone(),
            &browser_config,
        )?,
        BrowserSpawnMode::Local(session_id) => crate::browser::BrowserPanelState::start_with_session(
            seed.local_id.clone(),
            &browser_config,
            initial_url.clone(),
            session_id.as_deref(),
        )?,
        BrowserSpawnMode::Remote(request) => crate::browser::BrowserPanelState::start_remote(
            seed.local_id.clone(),
            &browser_config,
            initial_url.clone(),
            *request,
        )?,
        BrowserSpawnMode::RestoredRemote(target) => crate::browser::BrowserPanelState::restored_remote(
            seed.local_id.clone(),
            &browser_config,
            target,
            initial_url.clone(),
        ),
        // A panel of a browser host returns at once: the channel of the host
        // opens on its worker thread, and a host that never answers is a state
        // of the panel.
        BrowserSpawnMode::RemoteHost(backend) => crate::browser::BrowserPanelState::start_remote_host(
            seed.local_id.clone(),
            &browser_config,
            initial_url.clone(),
            &backend,
        ),
    };
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
        PanelKind::Editor
        | PanelKind::GitChanges
        | PanelKind::Usage
        | PanelKind::Browser
        | PanelKind::Device
        | PanelKind::SetupBoard => (String::new(), Vec::new()),
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
        AgentIntegrationKind::CodexMcp if uses_default_command => horizon_codex_mcp_args(crate::browser_host()),
        AgentIntegrationKind::GrokMcp
            if uses_default_command
                && !args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "--no-leader" | "--leader")) =>
        {
            vec!["--no-leader".to_string()]
        }
        AgentIntegrationKind::None | AgentIntegrationKind::CodexMcp | AgentIntegrationKind::GrokMcp => Vec::new(),
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

pub(super) fn agent_env(kind: PanelKind, local_id: &str, uses_default_command: bool) -> HashMap<String, String> {
    let mut env = HashMap::new();
    // Nested launches must not inherit another panel's work-hook identity.
    for key in crate::agent_work::WORK_ENV_KEYS {
        env.insert(key.to_owned(), String::new());
    }
    if kind.is_agent() {
        env.insert("HORIZON".to_string(), "1".to_string());
        env.insert("HORIZON_BROWSER_ACTOR".to_string(), browser_actor(local_id));
        env.insert(
            crate::browser::manifest::HOST_INSTANCE_ENV.to_string(),
            crate::browser::manifest::host_instance().to_string(),
        );
    }
    if kind == PanelKind::Grok {
        let command = uses_default_command
            .then(crate::browser_mcp_executable)
            .flatten()
            .and_then(|path| path.into_os_string().into_string().ok())
            .unwrap_or_default();
        env.insert("HORIZON_BROWSER_MCP_EXECUTABLE".to_string(), command);
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

/// The browser MCP entry of a Codex panel, as the `-c` arguments of `codex`.
/// `browser_host` is the host that the settings name, when there is one.
fn horizon_codex_mcp_args(browser_host: Option<&crate::BrowserHostConfig>) -> Vec<String> {
    let Some(command) = crate::browser_mcp_executable()
        .and_then(|path| path.into_os_string().into_string().ok())
        .and_then(|path| serde_json::to_string(&path).ok())
    else {
        tracing::warn!("could not resolve Horizon executable for browser MCP integration");
        return Vec::new();
    };
    let server_args = serde_json::to_string(&crate::browser_mcp_args(browser_host))
        .unwrap_or_else(|_| "[\"--browser-mcp\"]".to_string());
    vec![
        "-c".to_string(),
        format!("mcp_servers.horizon-browser.command={command}"),
        "-c".to_string(),
        format!("mcp_servers.horizon-browser.args={server_args}"),
        "-c".to_string(),
        "mcp_servers.horizon-browser.env_vars=[\"HORIZON_BROWSER_ACTOR\",\"HORIZON_BROWSER_HOST_INSTANCE\",\"HORIZON_HOME\"]"
            .to_string(),
        "-c".to_string(),
        "mcp_servers.horizon-browser.default_tools_approval_mode=\"approve\"".to_string(),
        "-c".to_string(),
        // Codex defaults to a 60s MCP tool timeout. `browser_handoff` waits for a
        // human for up to an hour; 3660s is that bound plus transport headroom.
        "mcp_servers.horizon-browser.tool_timeout_sec=3660".to_string(),
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
            PanelKind::Editor
            | PanelKind::GitChanges
            | PanelKind::Usage
            | PanelKind::Browser
            | PanelKind::Device
            | PanelKind::SetupBoard => 0,
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
        let env = agent_env(PanelKind::Codex, "panel-42", true);
        assert_eq!(env.get("HORIZON").map(String::as_str), Some("1"));
        assert_eq!(
            env.get("HORIZON_BROWSER_ACTOR").map(String::as_str),
            Some("horizon:panel-42")
        );
        assert_eq!(
            env.get(crate::browser::manifest::HOST_INSTANCE_ENV).map(String::as_str),
            Some(crate::browser::manifest::host_instance())
        );
        let shell_env = agent_env(PanelKind::Shell, "panel-42", true);
        for key in crate::agent_work::WORK_ENV_KEYS {
            assert_eq!(shell_env.get(key).map(String::as_str), Some(""));
        }
        assert!(!shell_env.contains_key("HORIZON_BROWSER_ACTOR"));
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
            "mcp_servers.horizon-browser.env_vars=[\"HORIZON_BROWSER_ACTOR\",\"HORIZON_BROWSER_HOST_INSTANCE\",\"HORIZON_HOME\"]"
        ));
        assert!(command.contains("mcp_servers.horizon-browser.default_tools_approval_mode=\"approve\""));
        assert!(command.contains("mcp_servers.horizon-browser.tool_timeout_sec=3660"));
        assert!(command.contains("--browser-mcp"));
        assert!(!command.contains("browser-cli"));
        #[cfg(target_os = "linux")]
        assert!(command.contains(&format!("/proc/{}/exe", std::process::id())));
    }

    #[test]
    fn a_named_browser_host_reaches_the_codex_entry_and_an_unnamed_keeps_the_local_server() {
        let local = horizon_codex_mcp_args(None).join(" ");
        assert!(
            local.contains("mcp_servers.horizon-browser.args=[\"--browser-mcp\"]"),
            "{local}"
        );
        assert!(!local.contains("--remote"), "no host means the local server: {local}");
        assert_eq!(
            local,
            horizon_codex_mcp_args(Some(&crate::BrowserHostConfig::default())).join(" "),
            "an entry that names no host is the entry of the local server"
        );

        let browser_host = crate::BrowserHostConfig {
            ssh_target: Some("browser-vm".to_string()),
            profile: Some("mail".to_string()),
        };
        let remote = horizon_codex_mcp_args(Some(&browser_host)).join(" ");
        assert!(
            remote.contains(
                "mcp_servers.horizon-browser.args=[\"--browser-mcp\",\"--remote\",\"browser-vm\",\"--profile\",\"mail\"]"
            ),
            "{remote}"
        );
        assert!(
            remote.contains("mcp_servers.horizon-browser.tool_timeout_sec=3660"),
            "the entry keeps the handoff timeout: {remote}"
        );
    }

    #[test]
    fn grok_browser_integration_stays_in_process_and_preserves_custom_launches() {
        let (_, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Grok,
            fresh_launch_context(&PanelResume::Fresh),
        );
        assert!(args.join(" ").contains("grok --no-leader"));
        let (_, already_set) = resolve_launch_command(
            None,
            vec!["--no-leader".to_string()],
            None,
            PanelKind::Grok,
            fresh_launch_context(&PanelResume::Fresh),
        );
        assert_eq!(already_set.join(" ").matches("--no-leader").count(), 1);
        let (_, explicit_leader) = resolve_launch_command(
            None,
            vec!["--leader".to_string()],
            None,
            PanelKind::Grok,
            fresh_launch_context(&PanelResume::Fresh),
        );
        assert!(explicit_leader.join(" ").contains("--leader"));
        assert!(!explicit_leader.join(" ").contains("--no-leader"));
        let (_, custom) = resolve_launch_command(
            Some("custom-grok".to_string()),
            vec!["custom-arg".to_string()],
            None,
            PanelKind::Grok,
            fresh_launch_context(&PanelResume::Fresh),
        );
        assert!(!custom.join(" ").contains("--no-leader"));
        assert_eq!(
            agent_env(PanelKind::Grok, "custom", false)["HORIZON_BROWSER_MCP_EXECUTABLE"],
            ""
        );
        let first = agent_env(PanelKind::Grok, "first", true);
        let second = agent_env(PanelKind::Grok, "second", true);
        assert_ne!(first["HORIZON_BROWSER_ACTOR"], second["HORIZON_BROWSER_ACTOR"]);
        assert!(!first["HORIZON_BROWSER_MCP_EXECUTABLE"].is_empty());
        assert!(!first.contains_key("GROK_HOME"));
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
        let claude_env = agent_env(PanelKind::Claude, "claude-panel", true);

        assert_eq!(
            claude_env
                .get("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN")
                .map(String::as_str),
            Some("1")
        );
        assert!(!agent_env(PanelKind::Codex, "codex-panel", true).contains_key("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"));
        assert!(!agent_env(PanelKind::Shell, "shell-panel", true).contains_key("CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN"));
    }

    #[test]
    fn shell_launch_args_adds_login_flag_when_requested() {
        assert_eq!(shell_launch_args(Vec::new(), true), vec!["-l".to_string()]);
    }

    #[test]
    fn disconnected_snapshot_launch_command_exits_without_reconnecting() {
        let (program, args) = disconnected_snapshot_launch_command();

        assert_eq!(program, default_shell());
        assert_eq!(args, vec!["-c".to_string(), "exit".to_string()]);
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

    fn browser_host(ssh_target: &str, profile: Option<&str>) -> crate::BrowserHostConfig {
        crate::BrowserHostConfig {
            ssh_target: Some(ssh_target.to_string()),
            profile: profile.map(str::to_string),
        }
    }

    /// A browser panel of the mode that the route chose, with the profile root
    /// of a temporary folder so the test leaves no browser state behind.
    fn spawn_routed_browser(mode: BrowserSpawnMode, initial_url: Option<String>) -> Panel {
        let root = tempfile::tempdir().expect("profile root");
        let mut options = PanelOptions {
            kind: PanelKind::Browser,
            browser_config: Some(crate::browser::BrowserConfig {
                command: Some(root.path().join("no-browser").to_string_lossy().into_owned()),
                profile_root: Some(root.path().join("profiles")),
                ..crate::browser::BrowserConfig::default()
            }),
            ..PanelOptions::default()
        };
        let seed = StaticPanelSeed::from_options(PanelId(1), WorkspaceId(1), "panel-mail".to_string(), &mut options);
        spawn_browser(seed, initial_url, options.browser_config.take(), mode).expect("browser panel")
    }

    #[test]
    fn a_named_browser_host_routes_a_new_panel_to_that_host_and_its_profile() {
        let host = browser_host("browser-vm", Some("mail"));
        let mode = browser_spawn_mode(BrowserSpawnMode::Local(None), Some(&host), true).expect("one SSH name");
        let started = std::time::Instant::now();
        let panel = spawn_routed_browser(mode, Some("https://example.test/".to_string()));
        let opened = started.elapsed();
        assert!(
            opened < std::time::Duration::from_millis(500),
            "a host that never answers does not hold the caller: {opened:?}"
        );
        let browser = panel.browser().expect("a browser panel");
        assert!(browser.is_remote_host(), "the panel uses the host of the settings");
        assert_eq!(browser.remote_host_target(), Some("browser-vm"));
        assert_eq!(
            browser.remote_host_profile(),
            Some("mail"),
            "the profile of the entry is the default target of the page"
        );
        assert_eq!(browser.kept_host_note(), Some(crate::browser::KEPT_HOST_NOTE));
        assert_eq!(
            panel.launch_command.as_deref(),
            Some("https://example.test/"),
            "the requested page is the first page of the host"
        );
        assert_eq!(
            panel.title, "example.test",
            "the title of the panel comes from the requested page"
        );
    }

    #[test]
    fn no_named_browser_host_keeps_the_spawn_path_of_the_local_browser() {
        assert!(
            crate::browser_host().is_none(),
            "no test of this binary names a browser host at start"
        );
        for entry in [None, Some(crate::BrowserHostConfig::default())] {
            let mode = browser_spawn_mode(BrowserSpawnMode::Local(None), entry.as_ref(), true)
                .expect("the local path never refuses");
            assert!(
                matches!(mode, BrowserSpawnMode::Local(None)),
                "no host, and an entry that names no host, is the local browser mode"
            );
        }

        let root = tempfile::tempdir().expect("profile root");
        let panel = spawn_panel(
            PanelId(2),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Browser,
                command: Some("https://example.test/".to_string()),
                browser_config: Some(crate::browser::BrowserConfig {
                    command: Some(root.path().join("no-browser").to_string_lossy().into_owned()),
                    profile_root: Some(root.path().join("profiles")),
                    ..crate::browser::BrowserConfig::default()
                }),
                ..PanelOptions::default()
            },
        )
        .expect("a local browser panel");
        let browser = panel.browser().expect("a browser panel");
        assert!(!browser.is_remote_host(), "a panel without a host stays local");
        assert_eq!(browser.remote_host_target(), None);
        assert_eq!(browser.remote_host_note(), None);
        assert_eq!(browser.kept_host_note(), None);
        assert_eq!(
            panel.launch_command.as_deref(),
            Some("https://example.test/"),
            "the initial URL of a local panel is unchanged"
        );
    }

    #[test]
    fn an_existing_browser_panel_keeps_its_backend_when_the_settings_name_a_host() {
        let host = browser_host("browser-vm", Some("mail"));

        // A saved panel builds its options without the flag, on purpose.
        let saved = crate::runtime_state::PanelState {
            kind: PanelKind::Browser,
            ..crate::runtime_state::PanelState::default()
        };
        assert!(
            !saved
                .to_panel_options(&crate::browser::BrowserConfig::default())
                .follows_browser_host,
            "a saved panel asks for no browser host"
        );
        let restored = browser_spawn_mode(
            BrowserSpawnMode::Local(None),
            Some(&host),
            saved
                .to_panel_options(&crate::browser::BrowserConfig::default())
                .follows_browser_host,
        )
        .expect("the saved mode");
        assert!(
            matches!(restored, BrowserSpawnMode::Local(None)),
            "a saved panel comes back on the backend it saved"
        );

        let shared = browser_spawn_mode(
            BrowserSpawnMode::Local(Some("profile-page-1".to_string())),
            Some(&host),
            true,
        )
        .expect("the mode of a duplicated page");
        assert!(
            matches!(shared, BrowserSpawnMode::Local(Some(id)) if id == "profile-page-1"),
            "a page of a local profile stays in its profile"
        );

        let provider = browser_spawn_mode(
            BrowserSpawnMode::RestoredRemote("target".to_string()),
            Some(&host),
            true,
        )
        .expect("the restored remote mode");
        assert!(
            matches!(provider, BrowserSpawnMode::RestoredRemote(target) if target == "target"),
            "a provider remote panel keeps its target"
        );
    }

    /// The options that `browser_requests` builds for a `browser_create` that
    /// names no remote target: the page of an agent, not a page of the person.
    fn agent_browser_create_options(root: &std::path::Path) -> PanelOptions {
        PanelOptions {
            command: Some("https://example.test/".to_string()),
            kind: PanelKind::Browser,
            visible: true,
            browser_config: Some(crate::browser::BrowserConfig {
                command: Some(root.join("no-browser").to_string_lossy().into_owned()),
                profile_root: Some(root.join("profiles")),
                ..crate::browser::BrowserConfig::default()
            }),
            ..PanelOptions::default()
        }
    }

    #[test]
    fn an_agent_browser_create_stays_local_when_the_settings_name_a_host() {
        let host = browser_host("browser-vm", Some("mail"));
        let root = tempfile::tempdir().expect("profile root");
        let options = agent_browser_create_options(root.path());
        assert!(
            !options.follows_browser_host,
            "a create of an agent asks for no browser host"
        );

        let mode = browser_spawn_mode(BrowserSpawnMode::Local(None), Some(&host), options.follows_browser_host)
            .expect("the agent path never refuses");
        assert!(
            matches!(mode, BrowserSpawnMode::Local(None)),
            "the page of an agent stays on the browser of this computer"
        );

        let panel = spawn_panel(PanelId(3), WorkspaceId(1), options).expect("an agent browser panel");
        let browser = panel.browser().expect("a browser panel");
        assert!(
            !browser.is_remote_host(),
            "the panel of an agent takes no page of the person's host"
        );
        assert_eq!(browser.remote_host_target(), None);
    }

    #[test]
    fn a_target_that_ssh_would_read_as_an_option_never_reaches_a_panel() {
        let host = browser_host("-oProxyCommand=touch /tmp/horizon-spawn-test", None);
        let error = browser_spawn_mode(BrowserSpawnMode::Local(None), Some(&host), true)
            .err()
            .expect("a target that ssh reads as an option is refused");
        assert!(error.to_string().contains("one SSH name"), "{error}");
    }

    #[test]
    fn retry_and_restart_of_a_panel_of_a_browser_host_keep_that_host() {
        let host = browser_host("browser-vm", Some("mail"));
        let mode = browser_spawn_mode(BrowserSpawnMode::Local(None), Some(&host), true).expect("the route");
        let mut panel = spawn_routed_browser(mode, Some("https://example.test/".to_string()));
        let browser = panel.browser_mut().expect("a browser panel");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while browser.remote_host_failure().is_none() && std::time::Instant::now() < deadline {
            browser.drain_events();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        browser.drain_events();
        assert!(
            browser.remote_host_failure().is_some(),
            "a host that does not answer ends the first channel with a typed reason"
        );

        let started = std::time::Instant::now();
        browser.relaunch();
        let retried = started.elapsed();
        assert!(
            retried < std::time::Duration::from_millis(500),
            "Retry never waits for ssh: {retried:?}"
        );
        assert_eq!(browser.status, crate::browser::BrowserStatus::Starting);
        assert!(browser.is_remote_host(), "Retry keeps the backend of the panel");
        assert_eq!(browser.remote_host_target(), Some("browser-vm"));
        assert_eq!(browser.remote_host_profile(), Some("mail"));
        assert_eq!(
            browser.remote_host_failure(),
            None,
            "the new channel starts with no reason of the old one"
        );
    }
}
