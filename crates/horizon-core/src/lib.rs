#![forbid(unsafe_code)]

mod attention;
mod board;
mod config;
mod config_migration;
pub mod dir_search;
mod editor;
mod error;
pub mod git_changes;
pub mod git_status;
pub mod git_watcher;
mod horizon_home;
mod layout;
mod panel;
mod remote_hosts;
mod runtime_state;
mod session_store;
mod shortcuts;
mod ssh;
mod terminal;
mod transcript;
mod usage_dashboard;
mod usage_stats;
mod view;
mod workspace;

pub use alacritty_terminal::index::Side as TerminalSide;
pub use alacritty_terminal::selection::SelectionType;
pub use attention::{AttentionId, AttentionItem, AttentionSeverity, AttentionState};
pub use board::{Board, ShutdownProgress, WorkspaceLayout};
pub use config::{
    Config, FeaturesConfig, OverlaysConfig, PresetConfig, ShortcutsConfig, TerminalConfig, WindowConfig,
    WorkspaceConfig,
};
pub use editor::{MarkdownEditor, PanelContent, PreviewMode};
pub use error::{Error, Result};
pub use git_changes::DiffViewer;
pub use git_status::{DiffHunk, DiffLine, DiffLineKind, FileChange, FileDiff, FileStatus, GitStatus};
pub use git_watcher::GitWatcher;
pub use horizon_home::HorizonHome;
pub use panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume};
pub use remote_hosts::{RemoteHost, RemoteHostCatalog, RemoteHostSources, RemoteHostStatus, discover_remote_hosts};
pub use runtime_state::{
    AgentSessionBinding, AgentSessionCatalog, AgentSessionRecord, DetachedWorkspaceState, PanelState, PanelTemplateRef,
    RuntimeState, WorkspaceState, WorkspaceTemplateRef, new_local_id,
};
pub use session_store::{
    ResolvedSession, SessionLease, SessionOpenDisposition, SessionStore, SessionSummary, StartupChooser,
    StartupDecision, StartupPromptReason,
};
pub use shortcuts::{AppShortcuts, ShortcutBinding, ShortcutKey, ShortcutModifiers};
pub use ssh::{DiscoveredSshHost, SshConnection, SshConnectionStatus, discover_ssh_hosts};
pub use terminal::{AgentNotification, Terminal, open_url};
pub use transcript::PanelTranscript;
pub use usage_dashboard::UsageDashboard;
pub use usage_stats::{DailyUsage, ToolUsage, UsageSnapshot, format_tokens};
pub use view::{CanvasViewState, DEFAULT_CANVAS_ZOOM, MAX_CANVAS_ZOOM, MIN_CANVAS_ZOOM, clamp_canvas_zoom};
pub use workspace::{WORKSPACE_COLORS, Workspace, WorkspaceId};
