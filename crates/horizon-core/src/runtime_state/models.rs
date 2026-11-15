use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

use crate::board::WorkspaceLayout;
use crate::config::{Config, TerminalConfig, WindowConfig, WorkspaceConfig};
use crate::panel::{PanelKind, PanelOptions, PanelResume};
use crate::ssh::SshConnection;

use super::{DEFAULT_COLS, DEFAULT_ROWS, new_local_id, normalize_cwd};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DetachedWorkspaceState {
    pub workspace_local_id: String,
    pub window: WindowConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct WorkspaceState {
    pub local_id: String,
    pub name: String,
    pub cwd: Option<String>,
    pub position: Option<[f32; 2]>,
    /// Where the new terminals of this workspace run. A workspace on this
    /// computer writes no field, so every saved workspace without one loads
    /// there.
    #[serde(default, skip_serializing_if = "crate::place::WorkspacePlace::is_local")]
    pub place: crate::place::WorkspacePlace,
    pub template: Option<WorkspaceTemplateRef>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_workspace_layout"
    )]
    pub layout: Option<WorkspaceLayout>,
    pub panels: Vec<PanelState>,
}

fn deserialize_workspace_layout<'de, D>(deserializer: D) -> std::result::Result<Option<WorkspaceLayout>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    let Some(value) = raw else {
        return Ok(None);
    };

    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "rows" => Ok(Some(WorkspaceLayout::Rows)),
        "columns" | "cols" => Ok(Some(WorkspaceLayout::Columns)),
        "grid" => Ok(Some(WorkspaceLayout::default())),
        "stack" | "cascade" => Ok(None),
        _ => Err(serde::de::Error::unknown_variant(
            &value,
            &["Rows", "Columns", "Grid", "Stack", "Cascade"],
        )),
    }
}

impl WorkspaceState {
    fn layout_from_config(workspace: &WorkspaceConfig) -> Option<WorkspaceLayout> {
        if workspace.terminals.iter().any(|panel| panel.position.is_some()) {
            None
        } else {
            Some(WorkspaceLayout::default())
        }
    }

    #[must_use]
    pub fn from_config(workspace_index: usize, workspace: &WorkspaceConfig, resolved_position: [f32; 2]) -> Self {
        let workspace_cwd = normalize_cwd(workspace.cwd.as_deref());
        let layout = Self::layout_from_config(workspace);
        let panels = workspace
            .terminals
            .iter()
            .enumerate()
            .map(|(panel_index, panel)| {
                PanelState::from_config(
                    workspace_index,
                    &workspace.name,
                    panel_index,
                    workspace,
                    resolved_position,
                    panel,
                )
            })
            .collect();

        Self {
            local_id: new_local_id(),
            name: workspace.name.clone(),
            cwd: workspace_cwd,
            position: Some(resolved_position),
            place: crate::place::WorkspacePlace::default(),
            template: Some(WorkspaceTemplateRef {
                workspace_index,
                workspace_name: workspace.name.clone(),
            }),
            layout,
            panels,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PanelState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_identity: Option<crate::browser::manifest::device::DeviceIdentity>,
    pub local_id: String,
    pub name: String,
    /// Whether `name` was explicitly chosen rather than generated. Missing
    /// values retain the legacy restore behavior for older runtime snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_is_custom: Option<bool>,
    pub kind: PanelKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
    pub rows: u16,
    pub cols: u16,
    pub resume: PanelResume,
    /// Permission to continue unfinished work, independent of conversation restore.
    pub work_resume: crate::agent_work::ResumePolicy,
    pub position: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
    pub session_binding: Option<AgentSessionBinding>,
    pub template: Option<PanelTemplateRef>,
    /// Scratch editor buffer content (persisted for file-less editors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_content: Option<String>,
    /// Profile root used when this Browser panel was launched. The outer
    /// option distinguishes a legacy snapshot from a panel launched with the
    /// default root, represented by `Some(BrowserProfileState { root: None })`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_profile: Option<BrowserProfileState>,
    /// Current URL of browser panels, restored as the navigation target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_url: Option<String>,
    /// The card that a block panel holds: its document, the revision the host
    /// assigned and the agent that owns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<crate::BlockPanelState>,
    /// The agent that opened this panel for the person in its workspace, as
    /// Horizon names that agent, so the titlebar can mark it after a restart.
    /// Only `open_terminal` and `open_panel` set it; a panel the person made
    /// keeps `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by: Option<String>,
    /// The agent panel that opened this panel, as its `horizon:<local id>`
    /// actor, so the pairs of the board name one panel also when two agent
    /// panels of one kind stand in a workspace. A session of an older build
    /// saved no actor, and keeps `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_by_actor: Option<String>,
    /// The machine of Horizon's machine list that this terminal connects to, so
    /// a restored panel attaches to the same tmux session and a click on the
    /// machine row focuses it instead of opening a second one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The tmux session that this terminal holds on the machine of its
    /// workspace's place, so a restart attaches to the same session and a close
    /// ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_session: Option<String>,
}

impl PanelState {
    /// The persisted session id, whether it came from a captured binding or
    /// an explicit `resume: session` setting.
    #[must_use]
    pub fn stored_session_id(&self) -> Option<&str> {
        self.session_binding
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .or(match &self.resume {
                PanelResume::Session { session_id } => Some(session_id.as_str()),
                PanelResume::Fresh | PanelResume::Last => None,
            })
    }

    pub(super) fn ensure_session_binding(&mut self, session_id: &str) -> bool {
        let mut changed = false;
        self.session_binding.get_or_insert_with(|| {
            changed = true;
            AgentSessionBinding::new(
                self.kind,
                session_id.to_string(),
                self.cwd.clone(),
                Some(self.name.clone()),
                None,
            )
        });
        changed
    }

    pub(super) fn replace_session_binding(&mut self, binding: AgentSessionBinding) -> bool {
        let resume_changed = if matches!(self.resume, PanelResume::Session { .. }) {
            let resume = PanelResume::Session {
                session_id: binding.session_id.clone(),
            };
            let changed = self.resume != resume;
            self.resume = resume;
            changed
        } else {
            false
        };
        let changed = resume_changed || self.session_binding.as_ref() != Some(&binding);
        self.session_binding = Some(binding);
        changed
    }

    #[must_use]
    pub fn from_config(
        workspace_index: usize,
        workspace_name: &str,
        panel_index: usize,
        workspace: &WorkspaceConfig,
        workspace_position: [f32; 2],
        panel: &TerminalConfig,
    ) -> Self {
        let position = panel
            .position
            .map(|relative| [workspace_position[0] + relative[0], workspace_position[1] + relative[1]]);
        let cwd = normalize_cwd(panel.cwd.as_deref()).or_else(|| normalize_cwd(workspace.cwd.as_deref()));
        let command = panel.command.clone();
        let args = panel.args.clone();
        let ssh_connection = panel.ssh_connection.clone();

        Self {
            local_id: new_local_id(),
            name: panel.name.clone(),
            name_is_custom: Some(!panel.name.is_empty()),
            kind: panel.kind,
            command: command.clone(),
            args: args.clone(),
            cwd: cwd.clone(),
            ssh_connection: ssh_connection.clone(),
            rows: panel.rows,
            cols: panel.cols,
            resume: panel.resume.clone(),
            work_resume: panel.work_resume.clone(),
            position,
            size: panel.size,
            session_binding: None,
            template: Some(PanelTemplateRef {
                workspace_index,
                workspace_name: workspace_name.to_string(),
                panel_index,
                kind: panel.kind,
                command,
                args,
                cwd,
                ssh_connection,
            }),
            device_identity: None,
            editor_content: None,
            browser_profile: None,
            browser_url: None,
            block: None,
            opened_by: None,
            opened_by_actor: None,
            machine: None,
            machine_session: None,
        }
    }

    pub(crate) fn browser_config_for_restore(
        &self,
        fallback: &crate::browser::BrowserConfig,
    ) -> crate::browser::BrowserConfig {
        let mut config = fallback.clone();
        if let Some(profile) = &self.browser_profile {
            config.profile_root.clone_from(&profile.root);
            if let Some(backend) = profile.backend {
                config.backend = backend;
            }
        }
        config
    }

    #[must_use]
    pub fn to_panel_options(&self, browser_config: &crate::browser::BrowserConfig) -> PanelOptions {
        let command = if self.kind == PanelKind::Browser {
            self.browser_url.clone().or_else(|| self.command.clone())
        } else {
            self.command.clone()
        };
        PanelOptions {
            device_identity: self.device_identity.clone(),
            // Not saved: a restored device panel has no viewer yet, and the
            // input is set again when the viewer attaches.
            device_input: crate::DeviceInput::default(),
            name: if self.name.is_empty() {
                None
            } else {
                Some(self.name.clone())
            },
            name_is_custom: self.name_is_custom,
            command,
            args: self.args.clone(),
            cwd: self.cwd.as_deref().map(Config::expand_tilde),
            ssh_connection: self.ssh_connection.clone(),
            rows: self.rows,
            cols: self.cols,
            kind: self.kind,
            resume: self.resume.clone(),
            work_resume: self.work_resume.clone(),
            position: self.position,
            size: self.size,
            visible: self.browser_profile.as_ref().is_none_or(|profile| !profile.hidden),
            local_id: Some(self.local_id.clone()),
            session_binding: self.session_binding.clone(),
            template: self.template.clone(),
            browser_session_id: self
                .browser_profile
                .as_ref()
                .and_then(|profile| profile.session_id.clone()),
            browser_private: self.browser_profile.as_ref().is_some_and(|profile| profile.private),
            browser_config: (self.kind == PanelKind::Browser).then(|| self.browser_config_for_restore(browser_config)),
            // A saved panel comes back on the backend that it saved, not on the
            // browser host that the settings of today name.
            follows_browser_host: false,
            // The rule of the host read the create of a new panel; a saved panel
            // carries the machine that served it instead.
            browser_request: None,
            browser_place: None,
            // Only the host of this app fills the entry of a panel that follows it.
            browser_host_entry: None,
            remote_session: None,
            remote_target: self
                .browser_profile
                .as_ref()
                .and_then(|profile| profile.remote_target.clone()),
            browser_host: self.browser_profile.as_ref().and_then(|profile| profile.host.clone()),
            transcript_root: None,
            restore_as_disconnected_snapshot: false,
            is_restore: true,
            launch_wrapper: Vec::new(),
            block: self.block.clone(),
            opened_by: self.opened_by.clone(),
            opened_by_actor: self.opened_by_actor.clone(),
            machine: self.machine.clone(),
            machine_session: self.machine_session.clone(),
        }
    }
}

impl Default for PanelState {
    fn default() -> Self {
        Self {
            local_id: String::new(),
            name: String::new(),
            name_is_custom: None,
            kind: PanelKind::default(),
            command: None,
            args: Vec::new(),
            cwd: None,
            ssh_connection: None,
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            resume: PanelResume::default(),
            work_resume: crate::agent_work::ResumePolicy::default(),
            position: None,
            size: None,
            session_binding: None,
            template: None,
            device_identity: None,
            editor_content: None,
            browser_profile: None,
            browser_url: None,
            block: None,
            opened_by: None,
            opened_by_actor: None,
            machine: None,
            machine_session: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct BrowserProfileState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<crate::browser::BackendKind>,
    /// Hidden browser panels stay live and controllable but do not render.
    #[serde(default)]
    pub hidden: bool,
    /// Configured remote target the panel ran at. A restored remote panel
    /// comes back stopped: its session ended with the previous run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_target: Option<String>,
    /// The browser host of another machine that served this panel, with the
    /// profile it used. A restart restores the panel on that host and profile;
    /// a live page cannot move between machines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<BrowserHostState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether the panel kept nothing of a profile: a private panel of this
    /// computer, or a private page of a browser host. A saved panel comes back
    /// private, so a restart keeps the kind of a panel.
    #[serde(default)]
    pub private: bool,
}

/// The browser host that served a panel, and the profile of that host.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct BrowserHostState {
    pub ssh_target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// The engine of that machine. A saved panel comes back on it, because its
    /// page cannot move to another engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<crate::config::BrowserHostEngine>,
    /// The program that starts the driver of that machine, when the entry named
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The login of a machine of the machine list, when the entry named one, so a
    /// saved panel comes back through the machine key of Horizon and not through
    /// the SSH configuration of the person. A live page cannot move to another
    /// machine, and a host of the person's own configuration records none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<crate::ssh::SshConnection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WorkspaceTemplateRef {
    pub workspace_index: usize,
    pub workspace_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PanelTemplateRef {
    pub workspace_index: usize,
    pub workspace_name: String,
    pub panel_index: usize,
    pub kind: PanelKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_connection: Option<SshConnection>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AgentSessionBinding {
    pub kind: PanelKind,
    pub session_id: String,
    pub cwd: Option<String>,
    pub label: Option<String>,
    pub updated_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AgentSessionKey {
    kind: PanelKind,
    session_id: String,
}

impl AgentSessionKey {
    #[must_use]
    pub fn new(kind: PanelKind, session_id: impl Into<String>) -> Self {
        Self {
            kind,
            session_id: session_id.into(),
        }
    }
}

impl AgentSessionBinding {
    #[must_use]
    pub fn same_saved_session_scope(previous: &[Self], current: &[Self]) -> bool {
        if previous.len() != current.len() {
            return false;
        }
        let mut counts = HashMap::with_capacity(previous.len());
        for binding in previous {
            *counts
                .entry((binding.kind, &binding.session_id, &binding.cwd))
                .or_insert(0_usize) += 1;
        }
        current.iter().all(|binding| {
            let Some(count) = counts.get_mut(&(binding.kind, &binding.session_id, &binding.cwd)) else {
                return false;
            };
            if *count == 0 {
                return false;
            }
            *count -= 1;
            true
        })
    }

    #[must_use]
    pub fn new(
        kind: PanelKind,
        session_id: String,
        cwd: Option<String>,
        label: Option<String>,
        updated_at: Option<i64>,
    ) -> Self {
        Self {
            kind,
            session_id,
            cwd,
            label,
            updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_comparison_ignores_order_and_metadata_but_preserves_duplicate_counts() {
        let previous: Vec<_> = (0..1000)
            .map(|index| {
                AgentSessionBinding::new(
                    PanelKind::Grok,
                    format!("session-{}", index / 2),
                    Some("/sample".into()),
                    None,
                    None,
                )
            })
            .collect();
        let mut current = previous.clone();
        current.reverse();
        for binding in &mut current {
            binding.label = Some("new metadata".into());
            binding.updated_at = Some(123);
        }
        assert!(AgentSessionBinding::same_saved_session_scope(&previous, &current));
        current[0].cwd = None;
        assert!(!AgentSessionBinding::same_saved_session_scope(&previous, &current));
        current = previous.clone();
        current[0] = current[2].clone();
        assert!(!AgentSessionBinding::same_saved_session_scope(&previous, &current));
        assert!(!AgentSessionBinding::same_saved_session_scope(
            &previous,
            &previous[1..]
        ));
        assert!(AgentSessionBinding::same_saved_session_scope(&[], &[]));
    }
}
