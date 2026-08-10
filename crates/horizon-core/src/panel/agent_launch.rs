#[cfg(any(windows, test))]
use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

use uuid::Uuid;

use crate::agents::agent_args_contain_session_directive;
use crate::error::{Error, Result};
use crate::horizon_home::HorizonHome;
use crate::runtime_state::AgentSessionBinding;
use crate::{AgentIntegrationKind, AgentResumeMode, agent_definition};

use super::{PanelKind, PanelOptions, PanelResume};

/// Internal entrypoint used to launch Windows script shims without routing a
/// generated prompt through a hand-built `cmd.exe` command line.
pub const AGENT_LAUNCH_HELPER_ARG: &str = "--horizon-agent-launch-helper";

/// Session-related inputs for building an agent launch command.
#[derive(Clone, Copy)]
pub(super) struct AgentLaunchContext<'a> {
    pub(super) resume: &'a PanelResume,
    pub(super) session_binding: Option<&'a AgentSessionBinding>,
    pub(super) should_resume_binding: bool,
    /// One-shot prompt for the initial agent process only. Restart and restore
    /// contexts always set this to `None`.
    pub(super) initial_agent_prompt: Option<&'a str>,
    /// Stable setup launch mode retained across restart and restore. It uses a
    /// login shell on Unix and Horizon's native process helper on Windows.
    pub(super) agent_login_shell: bool,
    /// True when reconnecting an existing panel (restore or restart) rather
    /// than launching a newly added one; continue-style agents only pass
    /// their continue flag when reconnecting.
    pub(super) is_restore: bool,
}

pub(super) fn validate_agent_launch_options(opts: &PanelOptions) -> Result<()> {
    if opts.agent_login_shell && !opts.kind.is_agent() {
        return Err(Error::Config(
            "agent_login_shell is only valid for agent panel kinds".to_string(),
        ));
    }
    let Some(_) = opts.initial_agent_prompt.as_deref() else {
        return Ok(());
    };
    if !opts.kind.is_agent() {
        return Err(Error::Config(
            "initial_agent_prompt is only valid for agent panel kinds".to_string(),
        ));
    }
    if agent_args_contain_session_directive(opts.kind, &opts.args) {
        return Err(Error::Config(
            "initial_agent_prompt cannot be combined with preset session or resume arguments".to_string(),
        ));
    }

    Ok(())
}

pub(super) fn resolve_agent_launch_command(
    command: Option<String>,
    args: Vec<String>,
    kind: PanelKind,
    launch: AgentLaunchContext<'_>,
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
            if launch.is_restore && matches!(launch.resume, PanelResume::Last) {
                launch_args.push(flag.to_string());
            }
        }
        AgentResumeMode::None => launch_args.extend(args),
    }

    if let Some(initial_agent_prompt) = launch.initial_agent_prompt {
        launch_args.push(initial_agent_prompt.to_string());
    }

    wrap_agent_command(program, launch_args, launch.agent_login_shell)
}

pub(super) fn wrap_agent_command(program: String, args: Vec<String>, agent_login_shell: bool) -> (String, Vec<String>) {
    #[cfg(windows)]
    if agent_login_shell {
        return windows_setup_agent_command(program, args, &windows_launch_helper());
    }

    let shell = default_shell();
    let mut command = vec![program];
    command.extend(args);
    let joined = command
        .iter()
        .map(|argument| shell_escape(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let shell_flags = if agent_login_shell { "-lic" } else { "-ic" };
    (shell, vec![shell_flags.to_string(), joined])
}

#[cfg(windows)]
fn windows_launch_helper() -> PathBuf {
    std::env::current_exe()
        .ok()
        .or_else(|| std::env::args_os().next().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("horizon.exe"))
}

#[cfg(any(windows, test))]
pub(super) fn windows_setup_agent_command(program: String, args: Vec<String>, helper: &Path) -> (String, Vec<String>) {
    let mut helper_args = vec![AGENT_LAUNCH_HELPER_ARG.to_string(), program];
    helper_args.extend(args);
    (format!("\"{}\"", helper.display()), helper_args)
}

pub(super) fn shell_escape(argument: &str) -> String {
    if !argument.is_empty()
        && argument
            .bytes()
            .all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'/' | b'.' | b'-'))
    {
        argument.to_string()
    } else {
        format!("'{}'", argument.replace('\'', "'\\''"))
    }
}

pub(super) const fn platform_default_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

/// Resolve the shell Horizon will use to wrap terminal and agent commands.
/// Empty or non-Unicode environment values cannot name a launchable program,
/// so they use the same platform fallback as a missing value.
#[must_use]
pub fn resolved_default_shell() -> String {
    resolve_default_shell(std::env::var("SHELL"))
}

pub(super) fn resolve_default_shell(shell: std::result::Result<String, std::env::VarError>) -> String {
    shell
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| platform_default_shell().to_string())
}

pub(super) fn default_shell() -> String {
    resolved_default_shell()
}

fn horizon_claude_plugin_args() -> Vec<String> {
    let path = HorizonHome::resolve().claude_plugin_dir();
    if path.is_dir() {
        vec!["--plugin-dir".to_string(), path.display().to_string()]
    } else {
        Vec::new()
    }
}
