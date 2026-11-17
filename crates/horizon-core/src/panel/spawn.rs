#[cfg(test)]
mod attention_standin_tests;
mod block;
mod browser;
mod device;
mod editor;
mod environment;
mod holder;
mod launch_arguments;
#[cfg(test)]
mod pi_attention_tests;
mod start;
mod terminal;

pub use browser::{BrowserMachine, browser_machine, browser_panel_of_options};
use browser::{browser_spawn_for, spawn_browser};
use editor::{spawn_editor, waiting_editor_panel};
pub use environment::{agent_env, browser_actor, replay_bytes_with_notice, wait_for_named_secrets};
pub use launch_arguments::{AgentLaunchContext, default_shell, resolve_launch_command};
// The platform shell name is read by the tests of this repository only.
#[cfg(test)]
pub use launch_arguments::platform_default_shell;
pub use start::{
    PanelStart, PendingRestart, PreparedLaunch, RestartPreflight, StartRequest, finish_restart, prepare_panel_start,
};
pub use start::{RestartRequest, prepare_restart};
pub(super) use terminal::{Placeholder, placeholder_panel, placeholder_terminal};
use terminal::{spawn_started_terminal, spawn_terminal, waiting_terminal_panel};

use super::work_resume::WorkPanel;
pub(crate) use super::work_resume::shutdown_for_restart;
#[cfg(test)]
use terminal::disconnected_snapshot_launch_command;
pub(crate) use terminal::{prepare_transcript_restore, resolve_spawn_launch};

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::agent_work::TitleState;
use crate::agents::AgentStatus;
use crate::editor::PanelContent;
use crate::error::Result;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef, claude_session_transcript_exists, new_local_id};
use crate::ssh::SshConnection;
use crate::transcript::PanelTranscript;
use crate::workspace::WorkspaceId;
use crate::{BlockPanelState, agent_definition};

use super::{
    AGENT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SCROLLBACK_LIMIT, DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind,
    PanelLayout, PanelOptions, PanelResume,
};
/// Starts a panel whose process waits for the preparation of a worker. A panel
/// that runs no local process starts at once, as it has nothing to prepare.
pub(crate) fn spawn_waiting_panel(id: PanelId, workspace_id: WorkspaceId, mut opts: PanelOptions) -> Result<Panel> {
    if opts.local_id.is_none() {
        opts.local_id = Some(new_local_id());
    }
    if opts.kind == PanelKind::Editor {
        return waiting_editor_panel(id, workspace_id, opts);
    }
    if !opts.waits_for_its_start() {
        return spawn_panel(id, workspace_id, opts);
    }
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);
    waiting_terminal_panel(id, workspace_id, local_id, opts)
}

/// Starts a panel with the preparation that a worker read.
pub(crate) fn spawn_started_panel(
    id: PanelId,
    workspace_id: WorkspaceId,
    opts: PanelOptions,
    start: PanelStart,
) -> Result<Panel> {
    let local_id = opts.local_id.clone().unwrap_or_else(new_local_id);
    spawn_started_terminal(id, workspace_id, local_id, opts, start)
}

struct StaticPanelSeed {
    id: PanelId,
    workspace_id: WorkspaceId,
    local_id: String,
    name: Option<String>,
    name_is_custom: Option<bool>,
    position: Option<[f32; 2]>,
    size: Option<[f32; 2]>,
    template: Option<PanelTemplateRef>,
    /// A panel that keeps nothing of a browser profile.
    browser_private: bool,
}

struct TerminalLaunchTrace<'a> {
    kind: PanelKind,
    resume: &'a PanelResume,
    session_binding: Option<&'a AgentSessionBinding>,
    should_resume_binding: bool,
    cwd: Option<&'a str>,
    cmd: String,
}

pub(crate) struct ResolvedTerminalLaunch {
    session_binding: Option<AgentSessionBinding>,
    program: String,
    launch_args: Vec<String>,
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
            browser_private: opts.browser_private,
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
            disconnected_browser_profile: None,
            disconnected_device_identity: None,
            session_binding: None,
            template: self.template,
            launched_at_millis: current_unix_millis(),
            has_custom_name,
            had_recent_output: false,
            agent_status: AgentStatus::default(),
            title_state: TitleState::default(),
            program_status: crate::agent_work::ProgramStatus::default(),
            last_output_at_millis: None,
            launch_command,
            launch_args: Vec::new(),
            launch_cwd,
            cwd_read: None,
            ssh_connection: None,
            opened_by: None,
            opened_by_actor: None,
            machine: None,
            machine_session: None,
            ssh_status: None,
            ssh_reconnect_attempt: 0,
            ssh_connected_at_millis: None,
            ssh_exit_recorded: false,
            ssh_end: None,
            program_end: None,
        }
    }
}

/// A working directory that a process of this computer cannot start in.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::panel) struct UnusableWorkingDirectory {
    /// The directory the panel asked for.
    pub(in crate::panel) path: String,
    /// Why a process cannot enter it.
    pub(in crate::panel) reason: String,
}

/// Refuse a working directory that a local process cannot enter.
///
/// A spawned process inherits Horizon's own working directory, and the PTY layer
/// ignores a `chdir` that fails. Without this check, a panel whose directory was
/// renamed or deleted runs its agent in Horizon's checkout instead of saying so.
/// `canonicalize` resolves every path component, so it fails for a missing path
/// and for one this process may not enter: the conditions that break `chdir`.
pub(in crate::panel) fn usable_working_directory(
    cwd: Option<&Path>,
) -> std::result::Result<(), UnusableWorkingDirectory> {
    crate::ui_thread::blocking("the working directory of a panel is resolved");
    let Some(path) = cwd.filter(|path| !path.as_os_str().is_empty()) else {
        return Ok(());
    };

    let resolved = path.canonicalize().map_err(|error| {
        let reason = match error.kind() {
            std::io::ErrorKind::NotFound => "the directory does not exist".to_string(),
            std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
            _ => error.to_string(),
        };
        unusable_working_directory(path, &reason)
    })?;
    if resolved.is_dir() {
        Ok(())
    } else {
        Err(unusable_working_directory(path, "the path is not a directory"))
    }
}

fn unusable_working_directory(path: &Path, reason: &str) -> UnusableWorkingDirectory {
    UnusableWorkingDirectory {
        path: path.display().to_string(),
        reason: reason.to_string(),
    }
}

pub(super) fn spawn_panel(id: PanelId, workspace_id: WorkspaceId, mut opts: PanelOptions) -> Result<Panel> {
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
        PanelKind::Block => {
            let state = opts
                .block
                .take()
                .unwrap_or_else(|| BlockPanelState::new(BlockPanelState::empty_document()));
            let seed = StaticPanelSeed::from_options(id, workspace_id, local_id, &mut opts);
            Ok(block::spawn_block(seed, state))
        }
        PanelKind::Browser => {
            let command = opts.command.take();
            let browser_config = opts.browser_config.take();
            let remote = browser_spawn_for(&mut opts)?;
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

#[must_use]
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

pub(super) fn scrollback_limit_for_kind(kind: PanelKind) -> usize {
    if kind.is_agent() {
        AGENT_PANEL_SCROLLBACK_LIMIT
    } else {
        match kind {
            PanelKind::Shell | PanelKind::Ssh | PanelKind::Command => DEFAULT_PANEL_SCROLLBACK_LIMIT,
            PanelKind::Editor | PanelKind::Browser | PanelKind::Device | PanelKind::Block => 0,
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

pub(crate) fn kitty_keyboard_for_kind(kind: PanelKind) -> bool {
    agent_definition(kind).is_none_or(|definition| definition.kitty_keyboard)
}

#[cfg(test)]
mod tests {
    use super::browser::{BrowserSpawnMode, browser_spawn_mode, restored_browser_mode};
    use super::environment::{EXECUTABLE_ENV, PANEL_IDENTITY_ENV_KEYS};
    use super::launch_arguments::{
        PLATFORM_USES_LOGIN_SHELL, codex_mcp_env_vars, horizon_codex_mcp_args, shell_launch_args,
    };
    use super::*;

    #[test]
    fn a_missing_working_directory_is_refused_with_its_path() {
        let state = tempfile::tempdir().expect("owned fixture state");
        let missing = state.path().join("renamed-away");

        let failure = usable_working_directory(Some(&missing)).expect_err("a missing directory is refused");

        assert_eq!(failure.path, missing.display().to_string());
        assert_eq!(failure.reason, "the directory does not exist");
    }

    #[test]
    fn a_file_working_directory_is_refused() {
        let state = tempfile::tempdir().expect("owned fixture state");
        let file = state.path().join("not-a-directory");
        std::fs::write(&file, b"x").expect("write fixture file");

        let failure = usable_working_directory(Some(&file)).expect_err("a file is not a working directory");

        assert_eq!(failure.path, file.display().to_string());
        assert_eq!(failure.reason, "the path is not a directory");
    }

    #[test]
    fn an_existing_directory_and_no_directory_are_usable() {
        let state = tempfile::tempdir().expect("owned fixture state");

        usable_working_directory(Some(state.path())).expect("an existing directory is usable");
        usable_working_directory(None).expect("no requested directory is usable");
        usable_working_directory(Some(Path::new(""))).expect("an empty path requests no directory");
    }

    #[test]
    fn a_remote_panel_needs_no_local_directory() {
        let ssh = PanelOptions {
            kind: PanelKind::Ssh,
            ..PanelOptions::default()
        };
        let shell = PanelOptions::default();
        let editor = PanelOptions {
            kind: PanelKind::Editor,
            ..PanelOptions::default()
        };

        assert!(!ssh.runs_a_local_process(), "an SSH panel runs its shell elsewhere");
        assert!(shell.runs_a_local_process(), "a shell runs in its directory here");
        assert!(editor.runs_a_local_process(), "an editor has no remote shell to reach");
    }

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
        assert_eq!(browser_actor(&"x".repeat(512)).len(), 24);
    }

    /// The process of a panel inherits the environment of the Horizon that starts
    /// it. A panel of another board is not this panel, and a value that names one
    /// would speak as it: every panel of a kind that is not an agent, and every
    /// agent panel before its own values, holds an empty identity instead.
    #[test]
    fn no_panel_keeps_the_identity_of_the_horizon_that_started_it() {
        for kind in [
            PanelKind::Shell,
            PanelKind::Ssh,
            PanelKind::Command,
            PanelKind::Codex,
            PanelKind::Claude,
            PanelKind::Pi,
            PanelKind::Grok,
            PanelKind::OpenCode,
        ] {
            let env = agent_env(kind, "panel-42", true);
            for key in PANEL_IDENTITY_ENV_KEYS {
                assert!(env.contains_key(key), "{kind:?} overrides the inherited {key}");
            }
            let expected = if kind.is_agent() {
                browser_actor("panel-42")
            } else {
                String::new()
            };
            assert_eq!(
                env.get(PANEL_IDENTITY_ENV_KEYS[0]).map(String::as_str),
                Some(expected.as_str()),
                "{kind:?} names its own panel, and a panel that is not an agent names none"
            );
        }
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
        assert!(
            command.contains(&format!(
                "mcp_servers.horizon-browser.env_vars={}",
                codex_mcp_env_vars()
            )),
            "the entry passes the identity, the home and the program of this Horizon: {command}"
        );
        assert!(
            codex_mcp_env_vars().contains("HORIZON_EXECUTABLE"),
            "a Codex panel hands the path of the running program to its MCP server"
        );
        assert!(command.contains("mcp_servers.horizon-browser.default_tools_approval_mode=\"approve\""));
        assert!(command.contains("mcp_servers.horizon-browser.tool_timeout_sec=3660"));
        assert!(command.contains("--browser-mcp"));
        assert!(!command.contains("browser-cli"));
        #[cfg(target_os = "linux")]
        assert!(command.contains(&format!("/proc/{}/exe", std::process::id())));
    }

    #[test]
    fn the_codex_entry_of_an_agent_names_the_local_server() {
        let entry = horizon_codex_mcp_args().join(" ");
        assert!(
            entry.contains("mcp_servers.horizon-browser.args=[\"--browser-mcp\"]"),
            "{entry}"
        );
        assert!(!entry.contains("--remote"), "the entry names no host: {entry}");
        assert!(
            entry.contains("mcp_servers.horizon-browser.tool_timeout_sec=3660"),
            "the entry keeps the handoff timeout: {entry}"
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

    /// Every agent panel names the program that runs Horizon, so a skill copies the
    /// binary of this computer instead of looking for `horizon` on a `PATH` that
    /// holds none when the person starts Horizon from a folder. A panel that is not
    /// an agent runs no skill, so it keeps none of its own.
    #[test]
    fn every_agent_panel_names_the_running_program() {
        for kind in [
            PanelKind::Codex,
            PanelKind::Claude,
            PanelKind::OpenCode,
            PanelKind::Gemini,
            PanelKind::KiloCode,
            PanelKind::Pi,
            PanelKind::Grok,
        ] {
            let env = agent_env(kind, "panel-42", true);
            let named = env
                .get(EXECUTABLE_ENV)
                .unwrap_or_else(|| panic!("{kind:?} names the program that runs it"));
            assert!(!named.is_empty(), "{kind:?} names one path");
            assert!(
                std::path::Path::new(named).is_file(),
                "{kind:?} names a file that this computer can copy: {named}"
            );
        }
        for kind in [
            PanelKind::Shell,
            PanelKind::Ssh,
            PanelKind::Command,
            PanelKind::Editor,
            PanelKind::Browser,
            PanelKind::Device,
            PanelKind::Block,
        ] {
            assert!(
                !agent_env(kind, "panel-42", true).contains_key(EXECUTABLE_ENV),
                "{kind:?} starts no skill, so it names no program of Horizon"
            );
        }
        assert_eq!(EXECUTABLE_ENV, "HORIZON_EXECUTABLE");
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
                "ConnectTimeout=10".to_string(),
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

    /// The place that the rule of `crate::browser::new_browser_panel` answers for
    /// a create that the browser host serves.
    const HOST: crate::browser::BrowserPlace = crate::browser::BrowserPlace::Host { private: false };

    fn browser_host(ssh_target: &str, profile: Option<&str>) -> crate::BrowserHostConfig {
        crate::BrowserHostConfig {
            ssh_target: Some(ssh_target.to_string()),
            profile: profile.map(str::to_string),
            ..crate::BrowserHostConfig::default()
        }
    }

    #[test]
    fn a_named_browser_host_routes_a_new_panel_to_that_host_and_its_profile() {
        let host = browser_host("browser-vm", Some("mail"));
        let mode =
            browser_spawn_mode(BrowserSpawnMode::Local(None), Some(HOST), Some(&host), None).expect("one SSH name");
        let BrowserSpawnMode::AttachedHost(attached) = mode else {
            panic!("a named host routes the panel to that host");
        };
        assert_eq!(attached.ssh_target(), "browser-vm");
        assert_eq!(
            attached.profile(),
            "mail",
            "the profile of the entry is the profile of the page"
        );

        let unnamed = browser_spawn_mode(
            BrowserSpawnMode::Local(None),
            Some(HOST),
            Some(&browser_host("browser-vm", None)),
            None,
        )
        .expect("one SSH name");
        let BrowserSpawnMode::AttachedHost(attached) = unnamed else {
            panic!("a host without a profile still routes the panel to that host");
        };
        assert_eq!(attached.profile(), crate::browser::DEFAULT_HOST_PROFILE);

        let title = crate::browser::panel_title_for_url("https://example.test/");
        assert_eq!(title, "example.test", "the title comes from the requested page");
    }

    #[test]
    fn no_named_browser_host_keeps_the_spawn_path_of_the_local_browser() {
        for entry in [None, Some(crate::BrowserHostConfig::default())] {
            let mode = browser_spawn_mode(BrowserSpawnMode::Local(None), None, entry.as_ref(), None)
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
        assert!(!browser.is_attached(), "a panel without a host runs on this computer");
        assert_eq!(browser.attached_host_target(), None);
        assert_eq!(
            panel.launch_command.as_deref(),
            Some("https://example.test/"),
            "the initial URL of a local panel is unchanged"
        );
    }

    /// The record of a rented machine of the list, and the login of a terminal of
    /// it with the machine key of this computer.
    fn machine_login() -> crate::ssh::SshConnection {
        let record = crate::Machine {
            name: "helsinki-1".to_string(),
            host: "10.0.0.5".to_string(),
            user: Some("peters".to_string()),
            port: Some(2222),
            ..crate::Machine::default()
        };
        let terminal = record.checked_terminal().expect("one route");
        let key = crate::MachineKeyRef {
            directory: "/h/.horizon/machine-keys".to_string(),
            name: "helsinki-1".to_string(),
        };
        crate::ssh::SshConnection::for_agent_terminal_with_key(&terminal, &key)
    }

    fn machine_browser_host() -> crate::BrowserHostConfig {
        crate::BrowserHostConfig {
            machine: Some("helsinki-1".to_string()),
            profile: Some("work".to_string()),
            ..crate::BrowserHostConfig::default()
        }
    }

    #[test]
    fn a_browser_host_of_a_machine_of_the_list_takes_the_login_of_that_machine() {
        let login = machine_login();
        let mode = browser_spawn_mode(
            BrowserSpawnMode::Local(None),
            Some(HOST),
            Some(&machine_browser_host()),
            Some(login.clone()),
        )
        .expect("the login of the machine");
        let BrowserSpawnMode::AttachedHost(attached) = mode else {
            panic!("a machine of the list routes the panel to that machine");
        };
        assert_eq!(
            attached.ssh_target(),
            "peters@10.0.0.5",
            "the target is the address and the user of the machine, which is what ssh reads"
        );
        assert_eq!(attached.profile(), "work");
        assert_eq!(
            attached.login_options(),
            login.login_option_args(),
            "every call of the panel carries the port, the key file and the known-hosts file of the machine"
        );
        assert_eq!(
            attached.login().map(crate::ssh::SshConnection::transport_target),
            Some("peters@10.0.0.5".to_string())
        );

        // A caller that resolved no login cannot reach such a machine: it holds no
        // key of the person, so the create says why instead of starting an `ssh` that
        // would ask for a password.
        let Err(error) = browser_spawn_mode(
            BrowserSpawnMode::Local(None),
            Some(HOST),
            Some(&machine_browser_host()),
            None,
        ) else {
            panic!("a machine of the list without a login was accepted");
        };
        assert!(error.to_string().contains("names the machine helsinki-1"), "{error}");
        assert!(error.to_string().contains("no login of it"), "{error}");
    }

    #[test]
    fn a_saved_panel_of_a_machine_of_the_list_comes_back_through_its_login() {
        let login = machine_login();
        let saved = crate::runtime_state::BrowserHostState {
            ssh_target: login.transport_target(),
            profile: Some("work".to_string()),
            login: Some(login.clone()),
            ..crate::runtime_state::BrowserHostState::default()
        };
        let mode = restored_browser_mode(BrowserSpawnMode::Local(None), Some(saved)).expect("the saved host");
        let BrowserSpawnMode::RestoredHost(attached) = mode else {
            panic!("a saved panel comes back on its browser host");
        };
        assert_eq!(attached.ssh_target(), "peters@10.0.0.5");
        assert_eq!(
            attached.login_options(),
            login.login_option_args(),
            "a restart keeps the machine key of the machine, and never asks the person's key"
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
        let saved_options = saved.to_panel_options(&crate::browser::BrowserConfig::default());
        assert!(
            !saved_options.follows_browser_host && saved_options.browser_host_entry.is_none(),
            "a saved panel asks for no browser host"
        );
        let restored = browser_spawn_mode(BrowserSpawnMode::Local(None), None, None, None).expect("the saved mode");
        assert!(
            matches!(restored, BrowserSpawnMode::Local(None)),
            "a saved panel comes back on the backend it saved"
        );

        let shared = browser_spawn_mode(
            BrowserSpawnMode::Local(Some("profile-page-1".to_string())),
            Some(crate::browser::BrowserPlace::ThisComputer {
                backend: crate::browser::BackendKind::ChromiumCdp,
            }),
            Some(&host),
            None,
        )
        .expect("the mode of a duplicated page");
        assert!(
            matches!(shared, BrowserSpawnMode::Local(Some(id)) if id == "profile-page-1"),
            "a page of a local profile stays in its profile"
        );

        let provider = browser_spawn_mode(
            BrowserSpawnMode::RestoredRemote("target".to_string()),
            None,
            Some(&host),
            None,
        )
        .expect("the restored remote mode");
        assert!(
            matches!(provider, BrowserSpawnMode::RestoredRemote(target) if target == "target"),
            "a provider remote panel keeps its target"
        );
    }

    #[test]
    fn a_saved_browser_host_and_profile_restore_the_panel_on_that_host() {
        let restored = restored_browser_mode(
            BrowserSpawnMode::Local(None),
            Some(crate::runtime_state::BrowserHostState {
                ssh_target: "browser-vm".to_string(),
                profile: Some("signed-in".to_string()),
                ..crate::runtime_state::BrowserHostState::default()
            }),
        )
        .expect("the restored host mode");
        let BrowserSpawnMode::RestoredHost(backend) = restored else {
            panic!("a saved host restores the panel on that host");
        };
        assert_eq!(backend.ssh_target(), "browser-vm");
        assert_eq!(backend.profile(), "signed-in");
    }

    #[test]
    fn a_saved_host_target_that_ssh_would_read_as_an_option_is_refused() {
        let refused = restored_browser_mode(
            BrowserSpawnMode::Local(None),
            Some(crate::runtime_state::BrowserHostState {
                ssh_target: "-oProxyCommand=evil".to_string(),
                profile: None,
                ..crate::runtime_state::BrowserHostState::default()
            }),
        );
        assert!(refused.is_err(), "a target that ssh reads as an option is refused");
    }

    #[test]
    fn a_saved_page_of_a_local_profile_keeps_its_profile() {
        let shared = restored_browser_mode(
            BrowserSpawnMode::Local(Some("profile-page-1".to_string())),
            Some(crate::runtime_state::BrowserHostState {
                ssh_target: "browser-vm".to_string(),
                profile: None,
                ..crate::runtime_state::BrowserHostState::default()
            }),
        )
        .expect("the mode of a restored shared page");
        assert!(
            matches!(shared, BrowserSpawnMode::Local(Some(id)) if id == "profile-page-1"),
            "a shared local page never moves to a host"
        );
    }

    #[test]
    fn an_agent_browser_create_follows_the_named_host() {
        let host = browser_host("browser-vm", Some("mail"));
        let mode =
            browser_spawn_mode(BrowserSpawnMode::Local(None), Some(HOST), Some(&host), None).expect("the agent route");
        let BrowserSpawnMode::AttachedHost(attached) = mode else {
            panic!("the page of an agent runs on the Chromium of the host");
        };
        assert_eq!(attached.ssh_target(), "browser-vm");
        assert_eq!(attached.profile(), "mail");
    }

    #[test]
    fn a_target_that_ssh_would_read_as_an_option_never_reaches_a_panel() {
        let host = browser_host("-oProxyCommand=touch /tmp/horizon-spawn-test", None);
        let error = browser_spawn_mode(BrowserSpawnMode::Local(None), Some(HOST), Some(&host), None)
            .err()
            .expect("a target that ssh reads as an option is refused");
        assert!(error.to_string().contains("one SSH name"), "{error}");
    }
}
