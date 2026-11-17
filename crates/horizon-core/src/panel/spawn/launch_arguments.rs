//! The program and the arguments that start a panel's process.
//!
//! Each agent kind answers its own arguments: the integration that Horizon adds
//! to the command of that agent, the subcommand or flag that names the
//! conversation that the panel resumes, and how the person's own command and
//! arguments take part. A shell or command panel answers the login shell that
//! this platform starts.

use uuid::Uuid;

use crate::horizon_home::HorizonHome;
use crate::runtime_state::AgentSessionBinding;
use crate::ssh::SshConnection;
use crate::{AgentIntegrationKind, AgentResumeMode, PanelKind, agent_definition};

use super::super::PanelResume;
use super::environment::EXECUTABLE_ENV;

/// Session-related inputs for building an agent launch command.
#[derive(Clone, Copy)]
pub struct AgentLaunchContext<'a> {
    pub resume: &'a PanelResume,
    pub session_binding: Option<&'a AgentSessionBinding>,
    pub should_resume_binding: bool,
    /// True when reconnecting an existing panel (restore or restart) rather
    /// than launching a newly added one; continue-style agents only pass
    /// their continue flag when reconnecting.
    pub is_restore: bool,
}

pub fn resolve_launch_command(
    command: Option<String>,
    args: Vec<String>,
    ssh_connection: Option<SshConnection>,
    kind: PanelKind,
    launch: AgentLaunchContext<'_>,
) -> (String, Vec<String>) {
    match kind {
        PanelKind::Editor | PanelKind::Browser | PanelKind::Device | PanelKind::Block => (String::new(), Vec::new()),
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
        AgentIntegrationKind::GrokMcp
            if uses_default_command
                && !args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "--no-leader" | "--leader")) =>
        {
            vec!["--no-leader".to_string()]
        }
        AgentIntegrationKind::ClaudePluginDir => horizon_claude_plugin_args(),
        _ => Vec::new(),
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

/// This copy differs from `crate::util::quote`: `~`, `*`, a backtick, a non-ASCII word and a
/// leading `-` stay bare.
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

pub(crate) fn shell_launch_args(args: Vec<String>, use_login_shell: bool) -> Vec<String> {
    if use_login_shell && args.is_empty() {
        vec!["-l".to_string()]
    } else {
        args
    }
}

pub(crate) const PLATFORM_USES_LOGIN_SHELL: bool = cfg!(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
));

pub const fn platform_default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

pub fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| platform_default_shell().to_string())
}

/// The variables of this Horizon that a Codex panel passes to the Horizon browser
/// MCP server: the identity of the panel, the home of this Horizon and the program
/// that runs it.
pub(crate) fn codex_mcp_env_vars() -> String {
    let names = [
        "HORIZON_BROWSER_ACTOR",
        crate::browser::manifest::HOST_INSTANCE_ENV,
        horizon_browser_control::paths::HOME_ENV,
        EXECUTABLE_ENV,
    ];
    serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_owned())
}

/// The browser MCP entry of a Codex panel, as the `-c` arguments of `codex`.
pub(crate) fn horizon_codex_mcp_args() -> Vec<String> {
    let Some(command) = crate::browser_mcp_executable()
        .and_then(|path| path.into_os_string().into_string().ok())
        .and_then(|path| serde_json::to_string(&path).ok())
    else {
        tracing::warn!("could not resolve Horizon executable for browser MCP integration");
        return Vec::new();
    };
    let server_args =
        serde_json::to_string(&crate::browser_mcp_args()).unwrap_or_else(|_| "[\"--browser-mcp\"]".to_string());
    vec![
        "-c".to_string(),
        format!("mcp_servers.horizon-browser.command={command}"),
        "-c".to_string(),
        format!("mcp_servers.horizon-browser.args={server_args}"),
        "-c".to_string(),
        format!("mcp_servers.horizon-browser.env_vars={}", codex_mcp_env_vars()),
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
