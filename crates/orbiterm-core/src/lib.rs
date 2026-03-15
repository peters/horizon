#![forbid(unsafe_code)]

mod attention;
mod board;
mod config;
mod error;
mod panel;
mod terminal;
mod workspace;

pub use attention::{AttentionId, AttentionItem, AttentionSeverity, AttentionState};
pub use board::Board;
pub use config::{Config, PresetConfig, ShortcutsConfig, TerminalConfig, WindowConfig, WorkspaceConfig};
pub use error::{Error, Result};
pub use panel::{DEFAULT_PANEL_SIZE, Panel, PanelId, PanelKind, PanelLayout, PanelOptions, PanelResume};
pub use terminal::Terminal;
pub use workspace::{WORKSPACE_COLORS, Workspace, WorkspaceId};
