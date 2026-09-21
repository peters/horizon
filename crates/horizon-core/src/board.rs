mod agent_status;
mod arrangement;
mod attention;
mod geometry;
mod shutdown;
mod workspaces;

pub use arrangement::WorkspaceAlignment;
#[cfg(feature = "cloud-workspaces")]
pub(crate) use arrangement::arranged_panel_layout;
use shutdown::FORCED_BROWSER_SHUTDOWN_WAIT;
pub use shutdown::{ForcedBrowserShutdownStatus, OrphanedRemoteHold, ShutdownProgress};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::attention::{AttentionItem, AttentionSeverity};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::panel::{Panel, PanelId, PanelKind, PanelOptions, PanelProcessActivity, PanelProcessOutput};
use crate::runtime_state::{PanelState, RuntimeState};
use crate::workspace::{Workspace, WorkspaceId};

const PANEL_CHROME_PAD: f32 = 8.0;
const PANEL_CHROME_TITLEBAR: f32 = 34.0;
const TERMINAL_PANEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const BROWSER_PANEL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const READY_FOR_INPUT_AUTO_DISMISS_AFTER: Duration = Duration::from_secs(45);
fn vec2_eq(left: [f32; 2], right: [f32; 2]) -> bool {
    (left[0] - right[0]).abs() <= f32::EPSILON && (left[1] - right[1]).abs() <= f32::EPSILON
}

fn panel_restore_options(
    panel_state: &PanelState,
    transcript_root: Option<&Path>,
    browser_config: &crate::browser::BrowserConfig,
) -> PanelOptions {
    let mut options = panel_state.to_panel_options(browser_config);
    options.transcript_root = transcript_root.map(Path::to_path_buf);
    options.restore_as_disconnected_snapshot = transcript_root.is_some() && panel_state.kind == PanelKind::Ssh;
    options
}

fn panel_restore_label(panel_state: &PanelState) -> String {
    if panel_state.name.is_empty() {
        panel_state.kind.display_name().to_string()
    } else {
        format!("'{}'", panel_state.name)
    }
}

/// Predefined layout arrangements for panels inside a workspace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum WorkspaceLayout {
    /// Single column, panels stacked top-to-bottom.
    Rows,
    /// Single row, panels side by side.
    Columns,
    /// Square-ish grid (auto columns).
    #[default]
    Grid,
}

impl WorkspaceLayout {
    pub const ALL: [Self; 3] = [Self::Rows, Self::Columns, Self::Grid];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rows => "Rows",
            Self::Columns => "Columns",
            Self::Grid => "Grid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceDockSide {
    Left,
    Right,
    Above,
    Below,
}

pub struct Board {
    pub panels: Vec<Panel>,
    pub workspaces: Vec<Workspace>,
    pub cloud_groups: crate::runtime_state::cloud_groups::CloudGroupsState,
    pub attention: Vec<AttentionItem>,
    panel_attention_signals: HashMap<PanelId, String>,
    /// Browser panels already removed from the board whose exact Chrome
    /// process is still retiring. Global shutdown must inherit these signals
    /// instead of losing them in detached cleanup work.
    retired_browser_shutdown_signals: Vec<crate::browser::BrowserShutdownSignal>,
    /// Providers of remote sessions whose teardown finished without the
    /// driver establishing the release. Each entry keeps counting against
    /// that provider's `max_sessions` for the rest of this run, independent
    /// of teardown cleanup, so a slot the provider may still hold is never
    /// handed out again on Horizon's own authority.
    unreleased_remote_holds: Vec<UnreleasedRemoteHold>,
    retained_empty_workspaces: HashSet<WorkspaceId>,
    pub focused: Option<PanelId>,
    pub active_workspace: Option<WorkspaceId>,
    pub attention_enabled: bool,
    next_panel_id: u64,
    next_workspace_id: u64,
    next_attention_id: u64,
}

/// A remote teardown that finished without an established release, kept so
/// the provider (by name and by cross-instance identity) stays counted.
#[derive(Clone, Debug)]
pub(crate) struct UnreleasedRemoteHold {
    pub(crate) provider: String,
    pub(crate) quota_key: Option<String>,
    pub(crate) recovery: Option<horizon_browser::RemoteAllocation>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoardProcessOutput {
    pub activity: PanelProcessActivity,
    pub cwd_changed: bool,
    pub persisted_state_changed: bool,
}

impl Board {
    #[must_use]
    pub fn new() -> Self {
        Self {
            panels: Vec::new(),
            workspaces: Vec::new(),
            cloud_groups: crate::runtime_state::cloud_groups::CloudGroupsState::default(),
            attention: Vec::new(),
            panel_attention_signals: HashMap::new(),
            retired_browser_shutdown_signals: Vec::new(),
            unreleased_remote_holds: Vec::new(),
            retained_empty_workspaces: HashSet::new(),
            focused: None,
            active_workspace: None,
            attention_enabled: false,
            next_panel_id: 1,
            next_workspace_id: 1,
            next_attention_id: 1,
        }
    }

    /// Build a board from a YAML config.
    ///
    /// # Errors
    ///
    /// Returns an error if the generated runtime state cannot be restored.
    pub fn from_config(config: &Config) -> Result<Self> {
        Self::from_runtime_state(&RuntimeState::from_config(config))
    }

    /// Build a board from a persisted runtime state snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime state cannot be restored.
    pub fn from_runtime_state(state: &RuntimeState) -> Result<Self> {
        Self::from_runtime_state_with_transcripts(state, None)
    }

    /// Build a board from a persisted runtime state snapshot and optional transcript root.
    ///
    /// # Errors
    ///
    /// Returns an error if the runtime state cannot be restored.
    pub fn from_runtime_state_with_transcripts(state: &RuntimeState, transcript_root: Option<&Path>) -> Result<Self> {
        Self::from_runtime_state_with_resume_limit(state, transcript_root, crate::agent_work::configured_resume_limit())
    }

    /// Restore with a maximum number of unattended work continuations.
    /// A zero limit asks for confirmation for every otherwise eligible panel.
    ///
    /// # Errors
    /// Returns an error if the runtime state cannot be restored.
    pub fn from_runtime_state_with_resume_limit(
        state: &RuntimeState,
        transcript_root: Option<&Path>,
        maximum: usize,
    ) -> Result<Self> {
        state.validate_remote_references()?;
        let _budget = crate::agent_work::RestoreBudget::new(maximum);
        let mut board = Self::new();

        for workspace_state in &state.workspaces {
            let ws_id = board.create_workspace_record(workspace_state);
            for panel_state in &workspace_state.panels {
                let options = panel_restore_options(panel_state, transcript_root, &state.browser);
                if crate::runtime_state::cloud_groups::managed_member(&state.cloud_groups, &panel_state.local_id) {
                    board.create_failed_restore_panel(
                        options,
                        ws_id,
                        if cfg!(feature = "cloud-workspaces") {
                            "Reconnecting cloud; remote processes continue independently"
                        } else {
                            "Cloud support is disabled; remote processes continue independently"
                        },
                    )?;
                    continue;
                }
                if let Err(error) = board.create_panel(options, ws_id) {
                    board.handle_panel_restore_failure(
                        &workspace_state.name,
                        panel_state,
                        ws_id,
                        transcript_root,
                        &state.browser,
                        &error,
                    );
                }
            }
            if let Some(workspace) = board.workspace_mut(ws_id) {
                workspace.layout = workspace_state.layout;
            }
        }

        if let Some(local_id) = &state.active_workspace_local_id
            && let Some(workspace_id) = board.workspace_id_by_local_id(local_id)
        {
            board.active_workspace = Some(workspace_id);
        }

        if let Some(local_id) = &state.focused_panel_local_id
            && let Some(panel_id) = board.panel_id_by_local_id(local_id)
            && board.panel(panel_id).is_some_and(|panel| panel.visible)
        {
            board.focused = Some(panel_id);
            board.active_workspace = board.panel_workspace_id(panel_id);
        } else {
            board.focused = board.panels.iter().find(|panel| panel.visible).map(|panel| panel.id);
        }

        board.cloud_groups = state.cloud_groups.clone();
        for local_id in crate::runtime_state::cloud_groups::workspace_ids(&state.cloud_groups) {
            if let Some(workspace) = board.workspace_id_by_local_id(local_id) {
                board.retained_empty_workspaces.insert(workspace);
            }
        }
        Ok(board)
    }

    fn handle_panel_restore_failure(
        &mut self,
        workspace_name: &str,
        panel_state: &PanelState,
        workspace_id: WorkspaceId,
        transcript_root: Option<&Path>,
        browser_config: &crate::browser::BrowserConfig,
        error: &Error,
    ) {
        let panel_label = panel_restore_label(panel_state);
        let error_message = error.to_string();
        tracing::error!(
            workspace = %workspace_name,
            panel = %panel_label,
            error = %error_message,
            "failed to restore panel"
        );

        let options = panel_restore_options(panel_state, transcript_root, browser_config);
        match self.create_failed_restore_panel(options, workspace_id, &error_message) {
            Ok(panel_id) => {
                self.create_attention(
                    workspace_id,
                    Some(panel_id),
                    "restore",
                    format!("Failed to restore {panel_label}: {error_message}"),
                    AttentionSeverity::High,
                );
            }
            Err(placeholder_error) => {
                tracing::error!(
                    workspace = %workspace_name,
                    panel = %panel_label,
                    error = %placeholder_error,
                    "failed to create restore failure placeholder"
                );
                self.create_attention(
                    workspace_id,
                    None,
                    "restore",
                    format!(
                        "Failed to restore {panel_label}: {error_message}. Also failed to show a placeholder: {placeholder_error}"
                    ),
                    AttentionSeverity::High,
                );
            }
        }
    }

    /// Restart a panel's terminal process in-place, preserving identity and
    /// session binding.
    ///
    /// # Errors
    ///
    /// Returns an error if the new terminal cannot be spawned.
    pub fn restart_panel(&mut self, id: PanelId) -> Result<()> {
        if self.panel(id).is_some_and(|panel| {
            crate::runtime_state::cloud_groups::managed_member(&self.cloud_groups, &panel.local_id)
        }) {
            return Err(Error::Config(
                "Reconnect the cloud to attach to its existing remote session".into(),
            ));
        }
        let panel = self
            .panels
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| Error::Pty(format!("panel {} not found", id.0)))?;
        panel.restart()
    }

    pub fn shutdown_terminal_panels(&mut self) {
        let shutdown = self.begin_async_shutdown();
        if !shutdown.wait_for_completion(TERMINAL_PANEL_SHUTDOWN_TIMEOUT) {
            tracing::warn!(
                completed = shutdown.panels_completed(),
                total = shutdown.panel_count(),
                "timed out waiting for panel shutdown"
            );
        }
        if !shutdown
            .wait_for_browser_shutdown(BROWSER_PANEL_SHUTDOWN_TIMEOUT.saturating_sub(shutdown.started_at().elapsed()))
        {
            tracing::warn!(
                forced_timeout_ms = FORCED_BROWSER_SHUTDOWN_WAIT.as_millis(),
                "failed to terminate all Chrome processes after the shared browser shutdown deadlines"
            );
        }
    }

    /// Begins shutting down all panels asynchronously.
    ///
    /// Sends shutdown signals to every terminal and spawns background threads
    /// to join their event loops. Returns a [`ShutdownProgress`] handle that
    /// can be polled each frame to track completion without blocking the UI.
    pub fn begin_async_shutdown(&mut self) -> ShutdownProgress {
        let completed = Arc::new(AtomicUsize::new(0));
        let mut panel_count = 0;
        let mut browser_shutdown_signals = Vec::new();

        for panel in &mut self.panels {
            if let Some(terminal) = panel.terminal_mut()
                && terminal.begin_work_shutdown(&completed)
            {
                panel_count += 1;
            } else {
                panel.request_shutdown();
            }
        }

        for panel in &mut self.panels {
            if let Some(terminal) = panel.terminal_mut()
                && terminal.begin_async_join(&completed)
            {
                panel_count += 1;
            }
            // Browser drivers tear down Chrome on their own threads; count
            // that teardown in the shutdown progress so the app cannot exit
            // while a driver still holds the profile lock.
            if let Some(signal) = panel.browser_shutdown_signal() {
                panel_count += 1;
                browser_shutdown_signals.push(signal);
            }
        }
        for signal in self.retired_browser_shutdown_signals.drain(..) {
            panel_count += 1;
            browser_shutdown_signals.push(signal);
        }

        // Unreleased holds leave with the teardowns: the progress keeps
        // counting them so a replacement board never sees the quota as free.
        ShutdownProgress::new(
            panel_count,
            completed,
            browser_shutdown_signals,
            std::mem::take(&mut self.unreleased_remote_holds),
        )
    }

    /// Allocations `provider` may still hold on this board: live remote
    /// panels whose release is not established, closed panels whose teardown
    /// is still retired here without an established release, and teardowns
    /// that finished this run without ever establishing it.
    #[must_use]
    pub fn remote_holds(&self, provider: &str) -> usize {
        self.count_remote_holds(
            |browser| browser.remote_provider() == Some(provider),
            |signal| signal.remote_provider() == Some(provider),
            |held| held.provider == provider,
        )
    }

    /// Allocations counted against one provider identity shared across
    /// Horizon instances (the quota key), whichever provider name the
    /// configuration gives it now.
    #[must_use]
    pub fn remote_holds_for_key(&self, key: &str) -> usize {
        self.count_remote_holds(
            |browser| browser.remote_quota_key() == Some(key),
            |signal| signal.remote_quota_key() == Some(key),
            |held| held.quota_key.as_deref() == Some(key),
        )
    }

    fn count_remote_holds(
        &self,
        live: impl Fn(&crate::browser::BrowserPanelState) -> bool,
        retired: impl Fn(&crate::browser::BrowserShutdownSignal) -> bool,
        unreleased: impl Fn(&UnreleasedRemoteHold) -> bool,
    ) -> usize {
        let live_count = self
            .panels
            .iter()
            .filter_map(|panel| panel.browser())
            .filter(|browser| live(browser) && browser.holds_remote_allocation())
            .count();
        let retired_count = self
            .retired_browser_shutdown_signals
            .iter()
            .filter(|signal| retired(signal) && signal.holds_remote_allocation())
            .count();
        let unreleased_count = self
            .unreleased_remote_holds
            .iter()
            .filter(|held| {
                unreleased(held)
                    && !held
                        .recovery
                        .as_ref()
                        .is_some_and(horizon_browser::RemoteAllocation::is_released)
            })
            .count();
        live_count + retired_count + unreleased_count
    }

    /// Drop finished teardowns, remembering the provider of any remote
    /// session whose release was never established so it keeps counting
    /// without keeping cleanup polling alive.
    fn sweep_retired_browser_shutdowns(&mut self) {
        self.unreleased_remote_holds.retain(|hold| {
            !hold
                .recovery
                .as_ref()
                .is_some_and(horizon_browser::RemoteAllocation::is_released)
        });
        let (complete, pending): (Vec<_>, Vec<_>) = self
            .retired_browser_shutdown_signals
            .drain(..)
            .partition(crate::browser::BrowserShutdownSignal::is_complete);
        self.retired_browser_shutdown_signals = pending;
        for signal in complete {
            if signal.holds_remote_allocation()
                && let Some(provider) = signal.remote_provider()
            {
                tracing::warn!(target: "browser", provider, "remote session ended without an established release; it keeps counting against the provider's limit");
                self.unreleased_remote_holds.push(UnreleasedRemoteHold {
                    provider: provider.to_string(),
                    quota_key: signal.remote_quota_key().map(str::to_string),
                    recovery: signal.remote_recovery(),
                });
            }
        }
    }

    /// Drain pending output from all panels. Returns `true` if any panel had activity.
    #[profiling::function]
    pub fn process_output(&mut self) -> BoardProcessOutput {
        self.sweep_retired_browser_shutdowns();
        let mut output = BoardProcessOutput::default();
        for panel in &mut self.panels {
            let panel_output: PanelProcessOutput = panel.process_output();
            output.activity.terminal |= panel_output.activity.terminal;
            output.activity.browser |= panel.visible && panel_output.activity.browser;
            output.cwd_changed |= panel_output.cwd_changed;
            output.persisted_state_changed |= panel_output.persisted_state_changed;
        }
        // Only run attention detection when terminals actually produced new
        // output.  The expensive path — `detect_attention()` — locks the
        // terminal mutex and iterates the full display, so skipping it on
        // idle frames is a significant CPU win.
        if self.attention_enabled && output.activity.terminal {
            self.update_attention();
        }
        // Working-status refresh runs every frame: the per-panel check is a
        // cheap timestamp compare for quiet panels, and the screen scan only
        // happens for panels with new output this frame. It is independent of
        // the attention-feed feature flag.
        self.update_agent_status();
        output
    }

    /// Whether a browser removed from the board still has process or profile
    /// cleanup in flight. The UI must keep polling output even when no panels
    /// remain so completion can be observed and retired state released.
    #[must_use]
    pub fn has_pending_browser_cleanup(&self) -> bool {
        !self.retired_browser_shutdown_signals.is_empty()
    }

    pub fn focus(&mut self, id: PanelId) {
        let Some(workspace_id) = self.panel_workspace_id(id) else {
            return;
        };
        let _ = self.set_panel_visible(id, true);
        self.focused = Some(id);
        self.active_workspace = Some(workspace_id);
    }

    pub fn focus_workspace(&mut self, id: WorkspaceId) {
        self.active_workspace = Some(id);
        self.focused = self.workspace(id).and_then(|workspace| {
            workspace
                .panels
                .iter()
                .rev()
                .copied()
                .find(|panel_id| self.panel(*panel_id).is_some_and(|panel| panel.visible))
        });
    }

    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|workspace| workspace.id == id)
    }

    #[must_use]
    pub fn panel(&self, id: PanelId) -> Option<&Panel> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    pub fn panel_mut(&mut self, id: PanelId) -> Option<&mut Panel> {
        self.panels.iter_mut().find(|panel| panel.id == id)
    }

    #[must_use]
    pub fn panel_workspace_id(&self, id: PanelId) -> Option<WorkspaceId> {
        self.panel(id).map(|panel| panel.workspace_id)
    }

    #[must_use]
    pub fn workspace_for_panel(&self, id: PanelId) -> Option<&Workspace> {
        self.panel_workspace_id(id)
            .and_then(|workspace_id| self.workspace(workspace_id))
    }

    #[must_use]
    pub fn workspace_id_by_local_id(&self, local_id: &str) -> Option<WorkspaceId> {
        self.workspaces
            .iter()
            .find(|workspace| workspace.local_id == local_id)
            .map(|workspace| workspace.id)
    }

    #[must_use]
    pub fn panel_id_by_local_id(&self, local_id: &str) -> Option<PanelId> {
        self.panels
            .iter()
            .find(|panel| panel.local_id == local_id)
            .map(|panel| panel.id)
    }
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
