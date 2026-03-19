use horizon_core::{PanelId, WorkspaceId};

/// Every dispatchable action in Horizon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandId {
    // Navigation
    SwitchWorkspace(WorkspaceId),
    FocusPanel(PanelId),

    // View
    ToggleSidebar,
    ToggleHud,
    ToggleMinimap,
    ToggleFullscreenWindow,
    ToggleFullscreenPanel,
    ResetView,
    ZoomIn,
    ZoomOut,
    AlignWorkspacesHorizontally,

    // Workspace / panel
    NewPanel,

    // Settings
    ToggleSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Workspace,
    Panel,
    Action,
}

impl Category {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workspace => "WORKSPACES",
            Self::Panel => "PANELS",
            Self::Action => "ACTIONS",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CommandEntry {
    pub id: CommandId,
    pub label: String,
    pub shortcut: Option<String>,
    /// Extra terms matched during filtering but not displayed.
    pub keywords: Vec<String>,
}

/// Build the static list of action commands (not workspace/panel -- those are
/// dynamic and assembled at query time by the palette).
pub fn action_commands(shortcut_prefix: &str) -> Vec<CommandEntry> {
    vec![
        CommandEntry {
            id: CommandId::NewPanel,
            label: "New Panel".into(),
            shortcut: Some(format!("{shortcut_prefix}+N")),
            keywords: vec!["create".into(), "terminal".into(), "add".into()],
        },
        CommandEntry {
            id: CommandId::ToggleSidebar,
            label: "Toggle Sidebar".into(),
            shortcut: Some(format!("{shortcut_prefix}+B")),
            keywords: vec!["sidebar".into(), "hide".into(), "show".into()],
        },
        CommandEntry {
            id: CommandId::ToggleHud,
            label: "Toggle HUD".into(),
            shortcut: Some(format!("{shortcut_prefix}+H")),
            keywords: vec!["heads".into(), "up".into(), "display".into(), "info".into()],
        },
        CommandEntry {
            id: CommandId::ToggleMinimap,
            label: "Toggle Minimap".into(),
            shortcut: Some(format!("{shortcut_prefix}+M")),
            keywords: vec!["overview".into(), "map".into()],
        },
        CommandEntry {
            id: CommandId::ToggleFullscreenWindow,
            label: "Toggle Fullscreen (Window)".into(),
            shortcut: Some(format!("{shortcut_prefix}+F11")),
            keywords: vec!["maximize".into(), "window".into(), "fullscreen".into()],
        },
        CommandEntry {
            id: CommandId::ToggleFullscreenPanel,
            label: "Toggle Fullscreen (Panel)".into(),
            shortcut: Some("F11".into()),
            keywords: vec!["maximize".into(), "panel".into(), "fullscreen".into(), "focus".into()],
        },
        CommandEntry {
            id: CommandId::ResetView,
            label: "Reset View".into(),
            shortcut: Some(format!("{shortcut_prefix}+0")),
            keywords: vec!["zoom".into(), "reset".into(), "fit".into()],
        },
        CommandEntry {
            id: CommandId::ZoomIn,
            label: "Zoom In".into(),
            shortcut: Some(format!("{shortcut_prefix}++")),
            keywords: vec!["zoom".into(), "bigger".into(), "enlarge".into()],
        },
        CommandEntry {
            id: CommandId::ZoomOut,
            label: "Zoom Out".into(),
            shortcut: Some(format!("{shortcut_prefix}+-")),
            keywords: vec!["zoom".into(), "smaller".into(), "shrink".into()],
        },
        CommandEntry {
            id: CommandId::AlignWorkspacesHorizontally,
            label: "Align Workspaces".into(),
            shortcut: Some(format!("{shortcut_prefix}+Shift+A")),
            keywords: vec!["arrange".into(), "horizontal".into(), "layout".into(), "row".into()],
        },
        CommandEntry {
            id: CommandId::ToggleSettings,
            label: "Settings".into(),
            shortcut: Some(format!("{shortcut_prefix}+,")),
            keywords: vec!["settings".into(), "config".into(), "preferences".into()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::{CommandId, action_commands};

    #[test]
    fn action_commands_have_unique_labels() {
        let entries = action_commands("Ctrl");
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        let mut deduped = labels.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(labels.len(), deduped.len(), "duplicate labels found");
    }

    #[test]
    fn action_commands_all_have_shortcuts() {
        for entry in action_commands("Ctrl") {
            assert!(entry.shortcut.is_some(), "entry '{}' has no shortcut", entry.label);
        }
    }

    #[test]
    fn action_commands_include_workspace_alignment() {
        let entries = action_commands("Ctrl");
        let entry = entries
            .iter()
            .find(|entry| entry.id == CommandId::AlignWorkspacesHorizontally)
            .expect("workspace alignment command");

        assert_eq!(entry.label, "Align Workspaces");
        assert_eq!(entry.shortcut.as_deref(), Some("Ctrl+Shift+A"));
    }
}
