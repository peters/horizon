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
pub use config::{Config, TerminalConfig, WorkspaceConfig};
pub use error::{Error, Result};
pub use panel::{Panel, PanelId, PanelOptions};
pub use terminal::Terminal;
pub use workspace::{WORKSPACE_COLORS, Workspace, WorkspaceId};
