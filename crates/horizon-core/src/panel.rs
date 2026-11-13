mod cwd;
mod echo;
mod lifecycle;
pub(crate) mod spawn;
mod work_resume;

pub use spawn::{
    PanelStart, PendingRestart, RestartPreflight, RestartRequest, StartRequest, finish_restart, prepare_panel_start,
    prepare_restart,
};
pub(crate) use spawn::{shutdown_for_restart, spawn_waiting_panel};

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::agent_definition;
use crate::agent_work::ResumePolicy;
use crate::agents::{AGENT_WORKING_SCAN_ROWS, AgentStatus, is_agent_working_line};
use crate::editor::{MarkdownEditor, PanelContent};
use crate::error::Result;
use crate::runtime_state::{AgentSessionBinding, PanelTemplateRef};
use crate::ssh::{SshConnection, SshConnectionStatus};
use crate::terminal::Terminal;
use crate::workspace::WorkspaceId;

use self::spawn::spawn_panel;
#[cfg(test)]
use self::spawn::{
    AgentLaunchContext, kitty_keyboard_for_kind, platform_default_shell, resolve_launch_command,
    scrollback_limit_for_kind,
};
pub use self::spawn::{browser_actor, current_unix_millis};

const DEFAULT_CELL_WIDTH: u16 = 8;
const DEFAULT_CELL_HEIGHT: u16 = 17;

pub const DEFAULT_PANEL_SIZE: [f32; 2] = [520.0, 340.0];

/// A new desktop viewer: the rows above its picture left the generic size's
/// body about 20 points high, so this keeps a 16:10 picture of 480 by 300.
pub const DEFAULT_DEVICE_PANEL_SIZE: [f32; 2] = [560.0, 540.0];
/// Lines of terminal scrollback kept for a shell, SSH, command, or agent panel.
pub const PANEL_SCROLLBACK_LIMIT: usize = 24_000;
const DEFAULT_PANEL_SCROLLBACK_LIMIT: usize = PANEL_SCROLLBACK_LIMIT;
const AGENT_PANEL_SCROLLBACK_LIMIT: usize = PANEL_SCROLLBACK_LIMIT;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PanelId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Shell,
    Ssh,
    Codex,
    Claude,
    OpenCode,
    Gemini,
    KiloCode,
    Pi,
    Grok,
    Command,
    Editor,
    Browser,
    Device,
    Block,
}

impl PanelKind {
    #[must_use]
    pub fn is_agent(self) -> bool {
        agent_definition(self).is_some()
    }

    /// Whether this panel owns a text-input surface that can receive typed
    /// or dictated text. Terminal-backed panels write to their PTY; editor
    /// panels insert at the caret; browser panels dispatch through CDP.
    #[must_use]
    pub const fn accepts_text_input(self) -> bool {
        !matches!(self, Self::Device | Self::Block)
    }

    #[must_use]
    pub fn supports_session_binding(self) -> bool {
        agent_definition(self).is_some_and(crate::AgentDefinition::supports_session_binding)
    }

    #[must_use]
    pub fn requires_exact_session_validation(self) -> bool {
        agent_definition(self).is_some_and(crate::AgentDefinition::requires_exact_session_validation)
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        if let Some(definition) = agent_definition(self) {
            return definition.display_name;
        }

        match self {
            Self::Shell => "Shell",
            Self::Ssh => "SSH",
            Self::Command => "Command",
            Self::Editor => "Editor",
            Self::Browser => "Browser",
            Self::Device => "Device",
            Self::Block => "Card",
            Self::Codex | Self::Claude | Self::OpenCode | Self::Gemini | Self::KiloCode | Self::Pi | Self::Grok => {
                unreachable!()
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
pub enum PanelResume {
    #[default]
    #[serde(rename = "fresh")]
    Fresh,
    #[serde(rename = "last")]
    Last,
    #[serde(rename = "session")]
    Session { session_id: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelLayout {
    pub position: [f32; 2],
    pub size: [f32; 2],
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            size: DEFAULT_PANEL_SIZE,
        }
    }
}

#[derive(Clone)]
pub struct PanelOptions {
    pub device_identity: Option<crate::browser::manifest::device::DeviceIdentity>,
    pub device_input: crate::DeviceInput,
    /// The card that a block panel holds; a block panel without one is empty.
    pub block: Option<crate::BlockPanelState>,
    pub name: Option<String>,
    /// Whether `name` was explicitly chosen by the user. `None` preserves
    /// the legacy behavior of treating every supplied name as custom.
    pub name_is_custom: Option<bool>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub ssh_connection: Option<SshConnection>,
    pub rows: u16,
    pub cols: u16,
    pub kind: PanelKind,
    pub resume: PanelResume,
    pub work_resume: ResumePolicy,
    pub position: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    /// Whether the panel participates in layout, rendering, and input.
    pub visible: bool,
    pub local_id: Option<String>,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    /// Active `browser` config section, so spawn honors `--config` and
    /// in-memory config edits instead of re-reading the default path.
    pub browser_config: Option<crate::browser::BrowserConfig>,
    /// Stable profile identity shared by explicitly duplicated browser pages.
    pub browser_session_id: Option<String>,
    /// Let the browser host that the settings name decide the backend of this
    /// panel. Only a panel that the person creates from the preset picker sets
    /// it: a page that an agent asks for, a page of a local profile that a panel
    /// duplicates and a saved panel keep the backend that their caller chose.
    pub follows_browser_host: bool,
    /// Run the browser panel at a remote grid with this prepared request
    /// instead of launching a local browser.
    pub remote_session: Option<horizon_browser::RemoteSessionRequest>,
    /// Configured remote target name for a browser panel restored from a
    /// previous run; it comes back stopped because its session ended then.
    pub remote_target: Option<String>,
    /// The browser host and profile that served a browser panel restored from a
    /// previous run. The panel comes back on that host, not on the local browser
    /// and not on the host that the settings of today name.
    pub browser_host: Option<crate::runtime_state::BrowserHostState>,
    pub transcript_root: Option<PathBuf>,
    pub restore_as_disconnected_snapshot: bool,
    /// True when this panel is being restored from persisted state rather
    /// than newly added; continue-style agents only reconnect on restore.
    pub is_restore: bool,
    /// A command line the terminal's program runs under, such as a systemd scope
    /// that holds every process the program starts; empty for none.
    pub launch_wrapper: Vec<String>,
    /// The agent that opened this panel for the person, as Horizon names that
    /// agent; `None` for a panel the person made.
    pub opened_by: Option<String>,
    /// The name of the machine in Horizon's machine list that this panel
    /// connects to, when it was opened from one. A click on the machine row
    /// focuses this panel instead of opening a second one, and the saved panel
    /// attaches to the same tmux session after a restart.
    pub machine: Option<String>,
    /// The tmux session that this panel runs on its machine, when the panel is a
    /// terminal of a workspace on a machine. Closing the panel ends it; a
    /// restart of Horizon attaches to it again.
    pub machine_session: Option<String>,
}

impl Default for PanelOptions {
    fn default() -> Self {
        Self {
            device_identity: None,
            device_input: crate::DeviceInput::default(),
            block: None,
            name: None,
            name_is_custom: None,
            command: None,
            args: Vec::new(),
            cwd: None,
            ssh_connection: None,
            rows: 24,
            cols: 80,
            kind: PanelKind::default(),
            resume: PanelResume::default(),
            work_resume: ResumePolicy::default(),
            position: None,
            size: None,
            visible: true,
            local_id: None,
            session_binding: None,
            template: None,
            browser_config: None,
            browser_session_id: None,
            follows_browser_host: false,
            remote_session: None,
            remote_target: None,
            browser_host: None,
            transcript_root: None,
            restore_as_disconnected_snapshot: false,
            is_restore: false,
            launch_wrapper: Vec::new(),
            opened_by: None,
            machine: None,
            machine_session: None,
        }
    }
}

/// Whether a panel of `kind` runs its process on this computer, where its working
/// directory decides where that process starts. A remote panel starts a shell
/// elsewhere, so a local directory that no longer exists must not stop it.
fn panel_runs_a_local_process(kind: PanelKind) -> bool {
    kind != PanelKind::Ssh
}

/// Whether a panel of `kind` starts a process of this computer, which the frame
/// starts only after a worker read its transcript, its directory and its saved
/// work.
#[must_use]
pub fn panel_waits_for_its_start(kind: PanelKind) -> bool {
    !matches!(kind, PanelKind::Device | PanelKind::Block | PanelKind::Browser)
}

impl PanelOptions {
    /// Whether the process of this panel waits for the preparation of a worker.
    #[must_use]
    pub fn waits_for_its_start(&self) -> bool {
        panel_waits_for_its_start(self.kind)
    }

    #[must_use]
    pub(crate) fn runs_a_local_process(&self) -> bool {
        if !panel_runs_a_local_process(self.kind) {
            return false;
        }
        true
    }
}

pub struct Panel {
    pub id: PanelId,
    pub local_id: String,
    pub title: String,
    pub terminal_title: String,
    pub kind: PanelKind,
    pub resume: PanelResume,
    pub work_resume: ResumePolicy,
    pub layout: PanelLayout,
    pub visible: bool,
    pub workspace_id: WorkspaceId,
    pub content: PanelContent,
    pub(crate) disconnected_browser_profile: Option<crate::runtime_state::BrowserProfileState>,
    pub(crate) disconnected_device_identity: Option<crate::browser::manifest::device::DeviceIdentity>,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    pub launched_at_millis: i64,
    has_custom_name: bool,
    /// Set by `process_output` each frame; read by agent status detection to skip
    /// the expensive `last_lines_text` scan for panels without new content.
    pub(crate) had_recent_output: bool,
    /// Working/idle state of the agent TUI, refreshed by
    /// `Board::update_agent_status` from the terminal's working indicator.
    pub(crate) agent_status: AgentStatus,
    last_output_at_millis: Option<i64>,
    /// Original launch command (for persistence).
    pub launch_command: Option<String>,
    /// Original launch args (for persistence).
    pub launch_args: Vec<String>,
    /// Working directory (for persistence): the directory that its process
    /// started in, or the last one that a worker read from it.
    pub launch_cwd: Option<PathBuf>,
    /// The process whose working directory a worker reads next, recorded when
    /// output may have changed it. A frame never reads the directory itself:
    /// on macOS that read starts `pgrep` and `lsof`.
    cwd_read: Option<u32>,
    /// Structured SSH connection metadata, when this is an SSH panel.
    pub ssh_connection: Option<SshConnection>,
    /// The agent that opened this panel for the person through
    /// `open_terminal`, as Horizon names that agent. The titlebar marks such a
    /// panel so a person can tell it from one they opened themselves.
    pub(crate) opened_by: Option<String>,
    /// The machine of Horizon's machine list that this terminal connects to.
    pub(crate) machine: Option<String>,
    /// The tmux session that this terminal runs on its machine, when it is a
    /// terminal of a workspace that is placed on one.
    pub(crate) machine_session: Option<String>,
    /// UI-facing SSH connection status for panels that remain visible after exit.
    pub ssh_status: Option<SshConnectionStatus>,
    /// The attempt that a reconnect of this panel reached since its last
    /// connection that lasted, so the mark of a lost link can name it.
    ssh_reconnect_attempt: u32,
    /// When the ssh connection of this panel came up, in milliseconds since the
    /// epoch, so a lost link reports how long it lasted.
    ssh_connected_at_millis: Option<i64>,
    /// Whether the end of the last ssh process was already recorded, so one end
    /// arms one attempt only.
    ssh_exit_recorded: bool,
    /// The end of the last ssh process, until the app reads it.
    ssh_end: Option<SshConnectionEnd>,
}

/// The end of one ssh process of a panel, as the app reads it once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SshConnectionEnd {
    /// The exit status of the process, when the system reported one.
    pub exit_code: Option<i32>,
    /// How long the connection was up before it ended; `None` when it never
    /// reached the machine.
    pub connected_for: Option<Duration>,
    /// The panel's saved command attaches a tmux session on a machine.
    pub machine_tied: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelProcessActivity {
    /// New terminal-grid output that may require agent scanning.
    pub terminal: bool,
    /// Browser state/frame activity that requires repainting but must not
    /// trigger terminal-grid scans.
    pub browser: bool,
    /// The terminal answered a program query, so the program may wait for
    /// its next answer even when the panel is not on the screen.
    pub answered_query: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PanelProcessOutput {
    pub activity: PanelProcessActivity,
    pub persisted_state_changed: bool,
}

impl Panel {
    /// Convenience accessor for the terminal content (if this panel holds one).
    #[must_use]
    pub fn terminal(&self) -> Option<&Terminal> {
        self.content.terminal()
    }

    /// Mutable accessor for the terminal content.
    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.content.terminal_mut()
    }

    #[must_use]
    pub fn ssh_status(&self) -> Option<SshConnectionStatus> {
        self.ssh_status
    }

    /// Keep the connection status of an SSH panel, and record the end of its
    /// ssh process once, so the app can decide whether to connect again.
    fn track_ssh_connection(&mut self, child_exited: bool, had_output: bool) {
        if child_exited {
            // The status of a panel whose process ended more than once is the
            // app's: it waits for an attempt, or it stays disconnected.
            if self.ssh_exit_recorded {
                return;
            }
            self.ssh_exit_recorded = true;
            let now = current_unix_millis();
            self.ssh_end = Some(SshConnectionEnd {
                exit_code: self
                    .content
                    .terminal()
                    .and_then(Terminal::child_exit_status)
                    .and_then(|status| status.code()),
                connected_for: self
                    .ssh_connected_at_millis
                    .map(|up| Duration::from_millis(u64::try_from(now.saturating_sub(up)).unwrap_or_default())),
                machine_tied: self.attaches_a_tmux_session(),
            });
            self.ssh_status = Some(SshConnectionStatus::Disconnected);
        } else if had_output && matches!(self.ssh_status, Some(SshConnectionStatus::Connecting)) {
            self.ssh_status = Some(SshConnectionStatus::Connected);
            self.ssh_connected_at_millis = Some(current_unix_millis());
            self.ssh_reconnect_attempt = 0;
        }
    }

    /// The end of this panel's ssh process, read once. `None` while its process
    /// runs, and after the app made its decision.
    #[must_use]
    pub fn take_ssh_end(&mut self) -> Option<SshConnectionEnd> {
        self.ssh_end.take()
    }

    /// Whether the saved command of this panel attaches to a tmux session on a
    /// machine, so Horizon can reach the same work again by itself.
    #[must_use]
    pub fn attaches_a_tmux_session(&self) -> bool {
        self.kind == PanelKind::Ssh
            && crate::place::attaches_a_tmux_session(
                self.ssh_connection
                    .as_ref()
                    .and_then(|connection| connection.remote_command.as_deref()),
            )
    }

    /// Marks this panel as waiting for its next attempt `attempt` after a lost
    /// link, and says how many attempts Horizon already made. The body keeps the
    /// last screen until the command starts again.
    pub fn show_reconnecting(&mut self, attempt: u32) {
        if self.kind != PanelKind::Ssh {
            return;
        }
        self.ssh_status = Some(SshConnectionStatus::Reconnecting);
        self.ssh_reconnect_attempt = attempt;
    }

    /// The attempt that the reconnect of this panel reached, while its mark is
    /// the one of a lost link that waits for the next one.
    #[must_use]
    pub fn ssh_status_attempt(&self) -> Option<u32> {
        matches!(self.ssh_status, Some(SshConnectionStatus::Reconnecting))
            .then_some(self.ssh_reconnect_attempt)
            .filter(|attempt| *attempt > 0)
    }

    /// Marks this panel as disconnected, after Horizon did not start its command
    /// again.
    pub fn show_disconnected(&mut self) {
        if self.kind == PanelKind::Ssh {
            self.ssh_status = Some(SshConnectionStatus::Disconnected);
            self.ssh_reconnect_attempt = 0;
        }
    }

    /// The agent that opened this panel for the person, as Horizon names it, or
    /// `None` for a panel the person made.
    #[must_use]
    pub fn opened_by(&self) -> Option<&str> {
        self.opened_by.as_deref()
    }

    /// The machine that this panel connects to, when it was opened from one.
    #[must_use]
    pub fn machine(&self) -> Option<&str> {
        self.machine.as_deref()
    }

    /// The tmux session that this terminal holds on its machine, when the
    /// workspace that holds it is placed on one.
    #[must_use]
    pub fn machine_session(&self) -> Option<&str> {
        self.machine_session.as_deref()
    }

    /// The best-effort end of this panel's tmux session: the panel's own ssh
    /// command line with the kill in place of its session command. `None` for
    /// every panel that holds no session of a machine.
    #[must_use]
    pub fn session_kill(&self) -> Option<crate::place::SessionKill> {
        let session = self.machine_session.as_deref()?;
        let connection = self.ssh_connection.as_ref()?;
        Some(crate::place::SessionKill::over(connection, session))
    }

    #[must_use]
    pub const fn had_recent_output(&self) -> bool {
        self.had_recent_output
    }

    #[must_use]
    pub fn had_recent_output_within(&self, window: Duration) -> bool {
        let window_millis = i64::try_from(window.as_millis()).unwrap_or(i64::MAX);
        self.last_output_at_millis
            .is_some_and(|last_output| current_unix_millis().saturating_sub(last_output) <= window_millis)
    }

    #[must_use]
    pub const fn agent_status(&self) -> AgentStatus {
        self.agent_status
    }

    /// Convenience accessor for the editor content (if this panel holds one).
    #[must_use]
    pub fn editor(&self) -> Option<&MarkdownEditor> {
        self.content.editor()
    }

    /// Mutable accessor for the editor content.
    pub fn editor_mut(&mut self) -> Option<&mut MarkdownEditor> {
        self.content.editor_mut()
    }

    /// Convenience accessor for the browser content (if this panel holds one).
    #[must_use]
    pub fn browser(&self) -> Option<&crate::browser::BrowserPanelState> {
        self.content.browser()
    }

    /// Preserve the selected engine even while a remote browser view is disconnected.
    #[must_use]
    pub fn browser_backend(&self) -> Option<crate::browser::BackendKind> {
        self.browser()
            .map(crate::browser::BrowserPanelState::backend)
            .or_else(|| {
                self.disconnected_browser_profile
                    .as_ref()
                    .and_then(|profile| profile.backend)
            })
    }

    /// Preserve remote identity while the view is a disconnected snapshot.
    #[must_use]
    pub fn browser_remote_target(&self) -> Option<&str> {
        self.browser()
            .and_then(crate::browser::BrowserPanelState::remote_target)
            .or_else(|| self.disconnected_browser_profile.as_ref()?.remote_target.as_deref())
    }

    /// Mutable accessor for the browser content.
    pub fn browser_mut(&mut self) -> Option<&mut crate::browser::BrowserPanelState> {
        self.content.browser_mut()
    }

    /// The read-only device viewer target, when this is a device panel.
    #[must_use]
    pub fn device(&self) -> Option<&crate::DevicePanelState> {
        self.content.device()
    }

    /// The card document and its host state, when this is a block panel.
    #[must_use]
    pub fn block(&self) -> Option<&crate::BlockPanelState> {
        self.content.block()
    }

    /// Mutable accessor for the card that this panel holds.
    pub fn block_mut(&mut self) -> Option<&mut crate::BlockPanelState> {
        self.content.block_mut()
    }

    /// Supplied machine labels survive a restore that has no connection yet.
    #[must_use]
    pub fn device_identity(&self) -> Option<&crate::browser::manifest::device::DeviceIdentity> {
        self.device()
            .and_then(|device| device.identity.as_ref())
            .or(self.disconnected_device_identity.as_ref())
    }
}

impl Panel {
    /// Spawn a new panel — either a PTY-backed terminal or a markdown editor.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal runtime cannot be created.
    pub fn spawn(id: PanelId, workspace_id: WorkspaceId, opts: PanelOptions) -> Result<Self> {
        spawn_panel(id, workspace_id, opts)
    }

    /// Wrap existing content without opening a PTY or browser driver.
    #[must_use]
    pub fn from_content(id: PanelId, workspace_id: WorkspaceId, kind: PanelKind, content: PanelContent) -> Self {
        Self {
            id,
            local_id: format!("panel-{}", id.0),
            title: kind.display_name().to_string(),
            terminal_title: String::new(),
            kind,
            resume: PanelResume::Fresh,
            work_resume: ResumePolicy::default(),
            layout: PanelLayout::default(),
            visible: true,
            workspace_id,
            content,
            disconnected_browser_profile: None,
            disconnected_device_identity: None,
            session_binding: None,
            template: None,
            launched_at_millis: 0,
            has_custom_name: false,
            had_recent_output: false,
            agent_status: AgentStatus::default(),
            last_output_at_millis: None,
            launch_command: None,
            launch_args: Vec::new(),
            launch_cwd: None,
            cwd_read: None,
            ssh_connection: None,
            opened_by: None,
            machine: None,
            machine_session: None,
            ssh_status: None,
            ssh_reconnect_attempt: 0,
            ssh_connected_at_millis: None,
            ssh_exit_recorded: false,
            ssh_end: None,
        }
    }

    /// Build a terminal-backed placeholder for a panel that failed to restore.
    ///
    /// # Errors
    ///
    /// Returns an error if the placeholder terminal runtime cannot be created.
    pub fn restore_failure(
        id: PanelId,
        workspace_id: WorkspaceId,
        opts: PanelOptions,
        error_message: &str,
    ) -> Result<Self> {
        spawn::placeholder_panel(
            id,
            workspace_id,
            opts,
            spawn::Placeholder::RestoreFailure(error_message),
        )
    }

    /// Follow the new directory of this workspace.
    ///
    /// A panel that runs in the directory of its workspace keeps that directory
    /// when the person sets another one. A panel with its own directory keeps
    /// its own directory. A panel that could not start says to set the directory
    /// of its workspace and restart it, which must then start the panel.
    pub(crate) fn follow_workspace_directory(
        &mut self,
        previous: Option<&std::path::Path>,
        directory: &std::path::Path,
    ) {
        if !panel_runs_a_local_process(self.kind) || self.launch_cwd.as_deref() != previous {
            return;
        }
        tracing::info!(
            panel_id = self.id.0,
            from = ?self.launch_cwd,
            to = %directory.display(),
            "panel follows the new directory of its workspace"
        );
        self.launch_cwd = Some(directory.to_path_buf());
    }

    /// The directory this panel cannot start its process in, if any.
    /// Starts the process of a panel that waited for the preparation of a
    /// worker, keeping its identity, its place and its visibility. A process
    /// that cannot start leaves the panel that says so.
    ///
    /// # Errors
    ///
    /// Returns an error if even the placeholder of a failed start cannot be
    /// created.
    pub fn apply_start(&mut self, options: &PanelOptions, start: spawn::PanelStart) -> Result<()> {
        if let Some(text) = start.editor_text {
            let text = text.map_err(crate::error::Error::Editor)?;
            if let Some(editor) = self.content.editor_mut() {
                editor.text = text;
                editor.dirty = false;
                editor.error = None;
            }
            return Ok(());
        }
        let mut started = match spawn::spawn_started_panel(self.id, self.workspace_id, options.clone(), start) {
            Ok(panel) => panel,
            Err(error) => {
                let message = error.to_string();
                tracing::error!(panel_id = self.id.0, %error, "failed to start the restored panel");
                spawn::placeholder_panel(
                    self.id,
                    self.workspace_id,
                    options.clone(),
                    spawn::Placeholder::RestoreFailure(&message),
                )?
            }
        };
        // The panel that waited keeps its place and its visibility: only its
        // content and the launch of its process change.
        started.visible = self.visible;
        started.layout = self.layout;
        std::mem::swap(self, &mut started);
        Ok(())
    }

    fn show_working_directory_failure(&mut self, failure: &spawn::UnusableWorkingDirectory) -> Result<()> {
        let Some((rows, cols)) = self.terminal().map(|terminal| (terminal.rows(), terminal.cols())) else {
            return Ok(());
        };
        let placeholder = spawn::Placeholder::for_working_directory(failure);
        let replacement = spawn::placeholder_terminal(self, rows, cols, placeholder)?;
        if let PanelContent::Terminal(mut old) =
            std::mem::replace(&mut self.content, PanelContent::Terminal(replacement))
        {
            old.request_shutdown();
        }
        Ok(())
    }

    /// Drain pending terminal events. Returns `true` if any output was processed.
    #[profiling::function]
    pub fn process_output(&mut self) -> PanelProcessOutput {
        if let Some(browser) = self.content.browser_mut() {
            let browser_output = browser.drain_events();
            // Feed page titles into the same runtime-title channel the
            // terminal xterm-title sequence uses, so the titlebar tracks
            // the page (custom names keep their "name — title" form).
            if browser.title != self.terminal_title {
                self.terminal_title = browser.title.clone();
            }
            self.had_recent_output = false;
            return PanelProcessOutput {
                activity: PanelProcessActivity {
                    terminal: false,
                    browser: browser_output.had_output,
                    answered_query: false,
                },
                persisted_state_changed: browser_output.url_changed || browser_output.config_changed,
            };
        }

        let should_track_live_cwd = matches!(self.kind, PanelKind::Shell | PanelKind::Command);
        let Some(terminal) = self.content.terminal_mut() else {
            self.had_recent_output = false;
            return PanelProcessOutput::default();
        };
        let had_output = terminal.process_events();
        let child_exited = terminal.child_exited();
        let answered_query = terminal.take_answered_query();
        let title_changed = self.terminal_title != terminal.title();
        if title_changed {
            self.terminal_title = terminal.title().to_string();
        }
        let child_pid = if had_output && should_track_live_cwd {
            terminal.child_pid()
        } else {
            None
        };
        self.had_recent_output = had_output;
        if had_output {
            self.last_output_at_millis = Some(current_unix_millis());
        }
        if self.kind == PanelKind::Ssh {
            self.track_ssh_connection(child_exited, had_output);
        }

        // The directory of the process is a read that can start a program, so
        // the worker answers it and a later frame keeps the value.
        if let Some(child_pid) = child_pid {
            self.cwd_read = Some(child_pid);
        }
        PanelProcessOutput {
            activity: PanelProcessActivity {
                terminal: had_output,
                browser: false,
                answered_query,
            },
            persisted_state_changed: false,
        }
    }

    /// The agent session this panel is currently bound to, if any — either
    /// the captured session binding or an explicit `resume: session` setting.
    /// Kinds without exact session binding (`Shell`, `Gemini`, `KiloCode`, …)
    /// never report one, even when a `resume: session` is configured.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        if !self.kind.supports_session_binding() {
            return None;
        }
        self.session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .or(match &self.resume {
                PanelResume::Session { session_id } => Some(session_id.as_str()),
                PanelResume::Fresh | PanelResume::Last => None,
            })
    }

    #[must_use]
    pub fn display_title(&self) -> Cow<'_, str> {
        if self.kind == PanelKind::Ssh {
            let base_title = if self.has_custom_name {
                self.ssh_connection
                    .as_ref()
                    .map(SshConnection::display_label)
                    .filter(|host_label| !self.title.contains(host_label))
                    .map_or_else(
                        || self.title.clone(),
                        |host_label| format!("{} ({host_label})", self.title),
                    )
            } else {
                self.title.clone()
            };

            if self.terminal_title.is_empty() || self.terminal_title == self.title || self.terminal_title == base_title
            {
                return Cow::Owned(base_title);
            }

            return Cow::Owned(format!("{base_title} — {}", self.terminal_title));
        }

        if self.has_custom_name {
            if self.terminal_title.is_empty() || self.terminal_title == self.title {
                Cow::Borrowed(&self.title)
            } else {
                Cow::Owned(format!("{} — {}", self.title, self.terminal_title))
            }
        } else if self.terminal_title.is_empty() {
            Cow::Borrowed(&self.title)
        } else {
            Cow::Borrowed(&self.terminal_title)
        }
    }

    #[must_use]
    pub fn name_is_custom(&self) -> bool {
        self.has_custom_name
    }

    #[must_use]
    pub fn child_exited(&self) -> bool {
        self.content.terminal().is_some_and(Terminal::child_exited)
    }

    #[must_use]
    pub fn rename(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }

        trimmed.clone_into(&mut self.title);
        self.has_custom_name = true;
        true
    }

    pub fn write_input(&mut self, bytes: &[u8]) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.write_input(bytes);
        }
    }

    /// Asks the content of this panel to stop. A dirty editor buffer only asks
    /// for its write here: the caller takes the text with
    /// [`Board::take_editor_saves`](crate::Board::take_editor_saves) and queues
    /// the write on the coordination worker, so no frame writes the file.
    pub fn request_shutdown(&mut self) {
        match &mut self.content {
            PanelContent::Terminal(terminal) => terminal.request_shutdown(),
            // The text of a dirty buffer goes to the coordination worker with
            // the frame that folds this panel away, and nothing waits here.
            PanelContent::Editor(editor) => editor.request_save(),
            PanelContent::Device(_) | PanelContent::Block(_) => {}
            PanelContent::Browser(browser) => browser.request_shutdown(),
        }
    }

    /// The write that this panel still owes its file, for a panel that leaves
    /// the board: no frame writes the text of a buffer that the person closed.
    #[must_use]
    pub(crate) fn take_pending_editor_write(&mut self) -> Option<crate::EditorSave> {
        let id = self.id;
        self.content.editor_mut()?.take_pending_write(id)
    }

    /// Take this panel's browser teardown-completion signal, if the browser
    /// shutdown was requested and not yet joined.
    #[must_use]
    pub(crate) fn browser_shutdown_signal(&mut self) -> Option<crate::browser::BrowserShutdownSignal> {
        if let Some(browser) = self.content.browser_mut() {
            browser.take_shutdown_signal()
        } else {
            None
        }
    }

    #[must_use]
    pub fn wait_for_shutdown(&mut self, timeout: Duration) -> bool {
        match &mut self.content {
            PanelContent::Terminal(terminal) => terminal.wait_for_shutdown(timeout),
            PanelContent::Editor(editor) => {
                editor.request_save();
                true
            }
            PanelContent::Device(_) | PanelContent::Block(_) => true,
            PanelContent::Browser(browser) => browser.shutdown_with_timeout(timeout),
        }
    }

    #[must_use]
    pub fn shutdown_with_timeout(&mut self, timeout: Duration) -> bool {
        match &mut self.content {
            PanelContent::Terminal(terminal) => terminal.shutdown_with_timeout(timeout),
            PanelContent::Editor(editor) => {
                editor.request_save();
                true
            }
            PanelContent::Device(_) | PanelContent::Block(_) => true,
            PanelContent::Browser(browser) => browser.shutdown_with_timeout(timeout),
        }
    }

    pub fn move_to(&mut self, position: [f32; 2]) {
        self.layout.position = position;
    }

    pub fn resize_layout(&mut self, size: [f32; 2]) {
        self.layout.size = size;
    }

    pub fn set_session_binding(&mut self, session_binding: Option<AgentSessionBinding>) {
        self.session_binding = session_binding;
    }

    pub fn scroll_scrollback_by(&mut self, delta: i32) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.scroll_scrollback_by(delta);
        }
    }

    pub fn set_scrollback(&mut self, scrollback: usize) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.set_scrollback(scrollback);
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.resize(rows, cols, cell_width, cell_height);
        }
    }

    pub fn resize_immediately(&mut self, rows: u16, cols: u16, cell_width: u16, cell_height: u16) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.resize_immediately(rows, cols, cell_width, cell_height);
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        if let Some(terminal) = self.content.terminal_mut() {
            terminal.set_focused(focused);
        }
    }

    /// Check whether this panel's agent TUI is rendering its working
    /// indicator (animated spinner plus a working status line pinned near the
    /// bottom of the screen, as Pi, Codex, Claude Code & co. do while busy).
    #[must_use]
    pub fn detect_agent_status(&self) -> AgentStatus {
        if !self.kind.is_agent() {
            return AgentStatus::Idle;
        }
        let Some(terminal) = self.content.terminal() else {
            return AgentStatus::Idle;
        };
        if terminal
            .bottom_lines_text(AGENT_WORKING_SCAN_ROWS)
            .iter()
            .any(|line| is_agent_working_line(line))
        {
            AgentStatus::Working
        } else {
            AgentStatus::Idle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_PANEL_SCROLLBACK_LIMIT, AgentLaunchContext, AgentSessionBinding, AgentStatus,
        DEFAULT_PANEL_SCROLLBACK_LIMIT, Panel, PanelContent, PanelId, PanelKind, PanelLayout, PanelResume,
        SshConnectionStatus, WorkspaceId, kitty_keyboard_for_kind, platform_default_shell, resolve_launch_command,
        scrollback_limit_for_kind,
    };
    use crate::ssh::SshConnection;

    fn test_panel(title: &str, terminal_title: &str, has_custom_name: bool) -> Panel {
        Panel {
            id: PanelId(1),
            local_id: "panel-1".to_string(),
            title: title.to_string(),
            terminal_title: terminal_title.to_string(),
            kind: PanelKind::Block,
            resume: PanelResume::Fresh,
            work_resume: crate::agent_work::ResumePolicy::default(),
            layout: PanelLayout::default(),
            visible: true,
            workspace_id: WorkspaceId(1),
            content: PanelContent::Block(crate::BlockPanelState::default()),
            disconnected_browser_profile: None,
            disconnected_device_identity: None,
            session_binding: None,
            template: None,
            launched_at_millis: 0,
            has_custom_name,
            had_recent_output: false,
            agent_status: AgentStatus::default(),
            last_output_at_millis: None,
            launch_command: None,
            launch_args: Vec::new(),
            launch_cwd: None,
            cwd_read: None,
            ssh_connection: None,
            opened_by: None,
            machine: None,
            machine_session: None,
            ssh_status: None,
            ssh_reconnect_attempt: 0,
            ssh_connected_at_millis: None,
            ssh_exit_recorded: false,
            ssh_end: None,
        }
    }

    #[test]
    fn a_panel_attaches_a_tmux_session_only_when_its_command_says_so() {
        let mut panel = test_panel("Pi", "", false);
        assert!(
            !panel.attaches_a_tmux_session(),
            "a panel that runs on this computer attaches no session"
        );

        panel.kind = PanelKind::Ssh;
        panel.ssh_connection = Some(SshConnection {
            host: "203.0.113.10".to_string(),
            remote_command: Some("bash -l".to_string()),
            ..SshConnection::default()
        });
        assert!(
            !panel.attaches_a_tmux_session(),
            "a remote shell is not a session of a machine"
        );

        panel.ssh_connection = Some(SshConnection {
            remote_command: Some(crate::place::session_command("hz-c8dfe3af-1")),
            ..SshConnection::default()
        });
        assert!(
            panel.attaches_a_tmux_session(),
            "a place terminal reaches the same session again"
        );
    }

    #[test]
    fn a_restart_keeps_the_screen_only_of_a_terminal_that_attaches_a_session() {
        use std::collections::HashMap;
        use std::time::{Duration, Instant};

        use crate::terminal::{Terminal, TerminalSpawnOptions};

        let mut terminal = Terminal::spawn(TerminalSpawnOptions {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "printf 'kept-line\\n'".to_string()],
            cwd: None,
            rows: 24,
            cols: 80,
            cell_width: 8,
            cell_height: 16,
            scrollback_limit: 256,
            window_id: 43,
            replay_bytes: Vec::new(),
            env: HashMap::new(),
            kitty_keyboard: true,
        })
        .expect("terminal should spawn");
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && terminal.viewport_text().first().is_none_or(|line| line != "kept-line") {
            let _ = terminal.process_events();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            terminal.viewport_text().first().map(String::as_str),
            Some("kept-line"),
            "the screen of the test shows the output"
        );
        let mut panel = Panel::from_content(
            PanelId(1),
            WorkspaceId(1),
            PanelKind::Command,
            PanelContent::Terminal(terminal),
        );

        assert!(
            panel.restart_replay_bytes().is_empty(),
            "a panel of this computer starts with an empty screen"
        );

        panel.kind = PanelKind::Ssh;
        panel.ssh_connection = Some(SshConnection {
            host: "203.0.113.10".to_string(),
            remote_command: Some(crate::place::session_command("hz-c8dfe3af-1")),
            ..SshConnection::default()
        });
        let kept = panel.restart_replay_bytes();
        assert!(
            String::from_utf8_lossy(&kept).contains("kept-line"),
            "a machine terminal that attaches a session keeps its screen: {kept:?}"
        );

        panel.ssh_connection = Some(SshConnection {
            host: "203.0.113.10".to_string(),
            remote_command: Some("bash -l".to_string()),
            ..SshConnection::default()
        });
        assert!(
            panel.restart_replay_bytes().is_empty(),
            "a remote shell without a session starts with an empty screen"
        );
    }

    #[test]
    fn a_waiting_terminal_says_that_it_reconnects_and_a_closed_one_does_not() {
        let mut panel = test_panel("Pi", "", false);
        panel.show_reconnecting(1);
        assert_eq!(panel.ssh_status(), None, "a panel of this computer shows no link");

        panel.kind = PanelKind::Ssh;
        panel.show_reconnecting(1);
        assert_eq!(panel.ssh_status(), Some(SshConnectionStatus::Reconnecting));
        panel.show_disconnected();
        assert_eq!(panel.ssh_status(), Some(SshConnectionStatus::Disconnected));
    }

    #[test]
    fn the_mark_of_a_reconnect_carries_the_attempt_that_it_reached() {
        let mut panel = test_panel("Pi", "", true);
        panel.kind = PanelKind::Ssh;

        for attempt in 1..=3 {
            panel.show_reconnecting(attempt);
            assert_eq!(panel.ssh_status_attempt(), Some(attempt), "attempt {attempt}");
        }

        panel.show_disconnected();
        assert_eq!(
            panel.ssh_status_attempt(),
            None,
            "a panel that stopped trying drops the attempt"
        );
    }

    #[test]
    fn session_id_prefers_captured_binding_over_explicit_resume() {
        let mut panel = test_panel("Pi", "", false);
        panel.kind = PanelKind::Pi;
        panel.resume = PanelResume::Session {
            session_id: "resume-1".to_string(),
        };
        panel.session_binding = Some(AgentSessionBinding::new(
            PanelKind::Pi,
            "bound-2".to_string(),
            None,
            None,
            None,
        ));

        assert_eq!(panel.session_id(), Some("bound-2"));
    }

    #[test]
    fn session_id_falls_back_to_explicit_resume() {
        let mut panel = test_panel("Pi", "", false);
        panel.kind = PanelKind::Pi;
        panel.resume = PanelResume::Session {
            session_id: "resume-1".to_string(),
        };

        assert_eq!(panel.session_id(), Some("resume-1"));
    }

    #[test]
    fn session_id_is_none_without_binding_or_resume() {
        let panel = test_panel("Shell", "", false);

        assert_eq!(panel.session_id(), None);
    }

    #[test]
    fn session_id_is_none_for_kinds_without_exact_binding() {
        for kind in [PanelKind::Shell, PanelKind::Gemini, PanelKind::KiloCode] {
            let mut panel = test_panel("Agent", "", false);
            panel.kind = kind;
            panel.resume = PanelResume::Session {
                session_id: "resume-1".to_string(),
            };

            assert_eq!(panel.session_id(), None, "kind {kind:?} must not report a session id");
        }
    }

    #[test]
    fn session_id_is_none_without_binding_or_resume_on_binding_kind() {
        let mut panel = test_panel("Pi", "", false);
        panel.kind = PanelKind::Pi;

        assert_eq!(panel.session_id(), None);
    }

    #[test]
    fn display_title_uses_runtime_title_for_unnamed_panels() {
        let panel = test_panel("Terminal 1", "Build running", false);

        assert_eq!(panel.display_title(), "Build running");
    }

    #[test]
    fn display_title_appends_runtime_title_for_custom_named_panels() {
        let panel = test_panel("Backend", "Build running", true);

        assert_eq!(panel.display_title(), "Backend — Build running");
    }

    #[test]
    fn display_title_omits_duplicate_runtime_title_for_custom_named_panels() {
        let panel = test_panel("Backend", "Backend", true);

        assert_eq!(panel.display_title(), "Backend");
    }

    #[test]
    fn process_output_copies_pinned_horizon_title_into_the_titlebar() {
        use std::collections::HashMap;
        use std::time::Duration;

        use alacritty_terminal::event::Event;

        use crate::terminal::{Terminal, TerminalSpawnOptions};

        let (program, args) = (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            vec!["-c".to_string(), "exit".to_string()],
        );
        let mut terminal = Terminal::spawn(TerminalSpawnOptions {
            program,
            args,
            cwd: None,
            rows: 24,
            cols: 80,
            cell_width: 8,
            cell_height: 16,
            scrollback_limit: 256,
            window_id: 42,
            replay_bytes: Vec::new(),
            env: HashMap::new(),
            kitty_keyboard: true,
        })
        .expect("terminal should spawn");
        assert!(
            terminal.shutdown_with_timeout(Duration::from_secs(2)),
            "test terminal should shut down before title injection"
        );
        let _ = terminal.process_events();
        terminal.handle_event(Event::Title(
            "HORIZON_TITLE:set:PR comments: Docker lifecycle".to_string(),
        ));
        terminal.handle_event(Event::Title(": horizon".to_string()));
        terminal.handle_event(Event::ResetTitle);

        let mut panel = Panel::from_content(
            PanelId(1),
            WorkspaceId(1),
            PanelKind::Codex,
            PanelContent::Terminal(terminal),
        );
        assert!(panel.rename("Codex"));

        let output = panel.process_output();
        assert_eq!(panel.display_title(), "Codex — PR comments: Docker lifecycle");
        assert!(
            !output.activity.terminal,
            "title copy after the PTY has exited must not count as grid output"
        );
    }

    #[test]
    fn display_title_keeps_ssh_host_visible_for_custom_named_panels() {
        let mut panel = test_panel("Prod API", "Connected", true);
        panel.kind = PanelKind::Ssh;
        panel.ssh_connection = Some(SshConnection {
            host: "prod-api".to_string(),
            user: Some("deploy".to_string()),
            ..SshConnection::default()
        });

        assert_eq!(panel.display_title(), "Prod API (deploy@prod-api) — Connected");
    }

    #[test]
    fn pi_kind_is_agent_with_display_name() {
        assert!(PanelKind::Pi.is_agent());
        assert!(PanelKind::Pi.supports_session_binding());
        assert_eq!(PanelKind::Pi.display_name(), "Pi");
    }

    #[test]
    fn text_panels_accept_dictation() {
        for kind in [
            PanelKind::Shell,
            PanelKind::Ssh,
            PanelKind::Codex,
            PanelKind::Command,
            PanelKind::Browser,
            PanelKind::Editor,
        ] {
            assert!(kind.accepts_text_input(), "kind {kind:?} must accept text input");
        }
        for kind in [PanelKind::Device, PanelKind::Block] {
            assert!(!kind.accepts_text_input(), "kind {kind:?} must reject text input");
        }
    }

    #[test]
    fn codex_without_exact_binding_starts_fresh() {
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Codex,
            AgentLaunchContext {
                resume: &PanelResume::Last,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        // Codex is launched via login shell without resume when no exact session is bound.
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("codex"));
        assert!(!args[1].contains("resume"));
    }

    #[test]
    fn claude_session_resume_uses_resume_flag() {
        let binding = AgentSessionBinding::new(PanelKind::Claude, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Claude,
            AgentLaunchContext {
                resume: &PanelResume::Session {
                    session_id: "session-42".to_string(),
                },
                session_binding: Some(&binding),
                should_resume_binding: true,
                is_restore: false,
            },
        );

        // Claude is launched via login shell: shell -lc "claude --resume session-42"
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--resume session-42"));
    }

    #[test]
    fn claude_fresh_without_binding_uses_ephemeral_session_id() {
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--dangerously-skip-permissions".to_string()],
            None,
            PanelKind::Claude,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--dangerously-skip-permissions"));
        assert!(
            args[1].contains("--session-id"),
            "fresh Claude panels must get an ephemeral --session-id"
        );
        assert!(!args[1].contains("--resume"));
    }

    #[test]
    fn restored_claude_fresh_binding_uses_resume_flag() {
        let binding = AgentSessionBinding::new(PanelKind::Claude, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--dangerously-skip-permissions".to_string()],
            None,
            PanelKind::Claude,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: Some(&binding),
                should_resume_binding: true,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("claude"));
        assert!(args[1].contains("--resume session-42"));
        assert!(!args[1].contains("--session-id session-42"));
    }

    #[test]
    fn codex_global_flags_come_before_exact_resume_subcommand() {
        let binding = AgentSessionBinding::new(PanelKind::Codex, "thread-7".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--no-alt-screen".to_string()],
            None,
            PanelKind::Codex,
            AgentLaunchContext {
                resume: &PanelResume::Last,
                session_binding: Some(&binding),
                should_resume_binding: true,
                is_restore: false,
            },
        );

        let cmd = &args[1];
        let flag_pos = cmd.find("--no-alt-screen").expect("flag present");
        let resume_pos = cmd.find("resume thread-7").expect("resume present");
        assert!(
            flag_pos < resume_pos,
            "global flags must precede resume subcommand: {cmd}"
        );
    }

    #[test]
    fn opencode_session_resume_uses_session_flag() {
        let binding = AgentSessionBinding::new(PanelKind::OpenCode, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--agent".to_string(), "build".to_string()],
            None,
            PanelKind::OpenCode,
            AgentLaunchContext {
                resume: &PanelResume::Session {
                    session_id: "session-42".to_string(),
                },
                session_binding: Some(&binding),
                should_resume_binding: true,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("opencode"));
        assert!(args[1].contains("--agent"));
        assert!(args[1].contains("build"));
        assert!(args[1].contains("--session session-42"));
    }

    #[test]
    fn opencode_fresh_without_binding_starts_without_session_flag() {
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--agent".to_string(), "build".to_string()],
            None,
            PanelKind::OpenCode,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("opencode"));
        assert!(args[1].contains("--agent"));
        assert!(args[1].contains("build"));
        assert!(!args[1].contains("--session"));
    }

    #[test]
    fn pi_fresh_without_binding_starts_without_session_flag() {
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Pi,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        assert_eq!(args, vec!["-ic".to_string(), "pi".to_string()]);
    }

    #[test]
    fn pi_session_resume_uses_session_flag_before_custom_args() {
        let binding = AgentSessionBinding::new(PanelKind::Pi, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--model".to_string(), "fast".to_string()],
            None,
            PanelKind::Pi,
            AgentLaunchContext {
                resume: &PanelResume::Session {
                    session_id: "session-42".to_string(),
                },
                session_binding: Some(&binding),
                should_resume_binding: true,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert_eq!(args[1], "pi --session session-42 --model fast");
    }

    #[test]
    fn grok_fresh_without_binding_starts_without_resume_flag() {
        let (_program, args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::Grok,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        assert_eq!(args, vec!["-ic".to_string(), "grok --no-leader".to_string()]);
    }

    #[test]
    fn grok_session_resume_uses_resume_flag_before_custom_args() {
        let binding = AgentSessionBinding::new(PanelKind::Grok, "session-42".to_string(), None, None, None);
        let (_program, args) = resolve_launch_command(
            None,
            vec!["-m".to_string(), "grok-build".to_string()],
            None,
            PanelKind::Grok,
            AgentLaunchContext {
                resume: &PanelResume::Session {
                    session_id: "session-42".to_string(),
                },
                session_binding: Some(&binding),
                should_resume_binding: true,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert_eq!(args[1], "grok --no-leader --resume session-42 -m grok-build");
    }

    #[test]
    fn gemini_panels_start_without_implicit_resume_flags() {
        let (_program, args) = resolve_launch_command(
            None,
            vec!["--model".to_string(), "gemini-2.5-pro".to_string()],
            None,
            PanelKind::Gemini,
            AgentLaunchContext {
                resume: &PanelResume::Fresh,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("gemini"));
        assert!(args[1].contains("--model"));
        assert!(!args[1].contains("resume"));
        assert!(!args[1].contains("--session"));
        assert!(!args[1].contains("--continue"));
    }

    #[test]
    fn kilocode_last_resume_uses_continue_flag_only_on_restore() {
        let (_program, restored_args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::KiloCode,
            AgentLaunchContext {
                resume: &PanelResume::Last,
                session_binding: None,
                should_resume_binding: false,
                is_restore: true,
            },
        );

        assert_eq!(restored_args.len(), 2);
        assert_eq!(restored_args[0], "-ic");
        assert!(restored_args[1].contains("kilo"));
        assert!(restored_args[1].contains("--continue"));

        let (_program, added_args) = resolve_launch_command(
            None,
            Vec::new(),
            None,
            PanelKind::KiloCode,
            AgentLaunchContext {
                resume: &PanelResume::Last,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        assert!(
            !added_args[1].contains("--continue"),
            "newly added KiloCode panels must start a fresh conversation: {}",
            added_args[1]
        );
    }

    #[test]
    fn explicit_command_wins_over_kind_defaults() {
        let (_program, args) = resolve_launch_command(
            Some("python".to_string()),
            vec!["-m".to_string(), "http.server".to_string()],
            None,
            PanelKind::Codex,
            AgentLaunchContext {
                resume: &PanelResume::Last,
                session_binding: None,
                should_resume_binding: false,
                is_restore: false,
            },
        );

        // Explicit command is still wrapped in login shell for agent kinds
        assert_eq!(args[0], "-ic");
        assert!(args[1].contains("python"));
        assert!(args[1].contains("-m"));
        assert!(args[1].contains("http.server"));
    }

    #[test]
    fn agent_panels_get_deeper_scrollback() {
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Shell),
            DEFAULT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Ssh),
            DEFAULT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Codex),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Claude),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::OpenCode),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::Gemini),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(
            scrollback_limit_for_kind(PanelKind::KiloCode),
            AGENT_PANEL_SCROLLBACK_LIMIT
        );
        assert_eq!(scrollback_limit_for_kind(PanelKind::Pi), AGENT_PANEL_SCROLLBACK_LIMIT);
        assert_eq!(scrollback_limit_for_kind(PanelKind::Grok), AGENT_PANEL_SCROLLBACK_LIMIT);
    }

    #[test]
    fn codex_panels_disable_kitty_keyboard_protocol() {
        assert!(!kitty_keyboard_for_kind(PanelKind::Codex));
        assert!(kitty_keyboard_for_kind(PanelKind::Claude));
        assert!(kitty_keyboard_for_kind(PanelKind::OpenCode));
        assert!(!kitty_keyboard_for_kind(PanelKind::Gemini));
        assert!(kitty_keyboard_for_kind(PanelKind::KiloCode));
        assert!(kitty_keyboard_for_kind(PanelKind::Pi));
        assert!(!kitty_keyboard_for_kind(PanelKind::Grok));
        assert!(kitty_keyboard_for_kind(PanelKind::Shell));
        assert!(kitty_keyboard_for_kind(PanelKind::Ssh));
    }

    #[test]
    fn platform_default_shell_matches_target() {
        let expected = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };

        assert_eq!(platform_default_shell(), expected);
    }
}

#[cfg(all(test, unix))]
mod launch_wrapper_tests {
    use super::*;

    #[test]
    fn a_launch_wrapper_runs_the_resolved_program_inside_it() {
        let temp = tempfile::tempdir().expect("temporary folder");
        let marker = temp.path().join("wrapped");
        let ran = temp.path().join("program-ran");
        let panel = Panel::spawn(
            PanelId(1),
            WorkspaceId(1),
            PanelOptions {
                kind: PanelKind::Command,
                command: Some("touch".to_owned()),
                args: vec![ran.display().to_string()],
                // The wrapper leaves a mark, then runs the program with its arguments.
                launch_wrapper: vec![
                    "sh".to_owned(),
                    "-c".to_owned(),
                    "touch \"$0\"; exec \"$@\"".to_owned(),
                    marker.display().to_string(),
                ],
                ..PanelOptions::default()
            },
        )
        .expect("panel starts");
        let started = std::time::Instant::now();
        while !(marker.exists() && ran.exists()) && started.elapsed() < std::time::Duration::from_secs(10) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(panel);
        assert!(marker.exists(), "the wrapper ran");
        assert!(ran.exists(), "and ran the program with its arguments");
    }
}
