#![forbid(unsafe_code)]

mod attention;
mod board;
mod config;
pub mod dir_search;
mod editor;
mod error;
mod panel;
mod runtime_state;
mod terminal;
mod transcript;
mod workspace;

pub use alacritty_terminal::index::Side as TerminalSide;
pub use alacritty_terminal::selection::SelectionType;
pub use attention::{AttentionId, AttentionItem, AttentionSeverity, AttentionState};
pub use board::{Board, WorkspaceLayout};
pub use config::{Config, PresetConfig, ShortcutsConfig, TerminalConfig, WindowConfig, WorkspaceConfig};
pub use editor::{MarkdownEditor, PanelContent, PreviewMode};
pub use error::{Error, Result};
pub use panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume};
pub use runtime_state::{
    AgentSessionBinding, AgentSessionCatalog, AgentSessionRecord, PanelState, PanelTemplateRef, RuntimeState,
    WorkspaceState, WorkspaceTemplateRef, new_local_id, new_session_binding, runtime_state_path_for_config,
    transcript_root_path_for_config,
};
pub use terminal::Terminal;
pub use transcript::PanelTranscript;
pub use workspace::{WORKSPACE_COLORS, Workspace, WorkspaceId};
