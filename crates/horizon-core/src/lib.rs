#![forbid(unsafe_code)]

mod attention;
mod board;
mod config;
pub mod dir_search;
mod editor;
mod error;
pub mod git_changes;
pub mod git_status;
pub mod git_watcher;
mod layout;
mod panel;
mod runtime_state;
mod terminal;
mod transcript;
mod usage_dashboard;
mod usage_stats;
mod workspace;

pub use alacritty_terminal::index::Side as TerminalSide;
pub use alacritty_terminal::selection::SelectionType;
pub use attention::{AttentionId, AttentionItem, AttentionSeverity, AttentionState};
pub use board::{Board, WorkspaceLayout};
pub use config::{
    Config, OverlaysConfig, PresetConfig, ShortcutsConfig, TerminalConfig, WindowConfig, WorkspaceConfig,
};
pub use editor::{MarkdownEditor, PanelContent, PreviewMode};
pub use error::{Error, Result};
pub use git_changes::GitChangesViewer;
pub use git_status::{DiffHunk, DiffLine, DiffLineKind, FileChange, FileDiff, FileStatus, GitStatus};
pub use git_watcher::GitWatcher;
pub use panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume};
pub use runtime_state::{
    AgentSessionBinding, AgentSessionCatalog, AgentSessionRecord, PanelState, PanelTemplateRef, RuntimeState,
    WorkspaceState, WorkspaceTemplateRef, new_local_id, runtime_state_path_for_config, transcript_root_path_for_config,
};
pub use terminal::{AgentNotification, Terminal};
pub use transcript::PanelTranscript;
pub use usage_dashboard::UsageDashboard;
pub use usage_stats::{DailyUsage, ToolUsage, UsageSnapshot, format_tokens};
pub use workspace::{WORKSPACE_COLORS, Workspace, WorkspaceId};
