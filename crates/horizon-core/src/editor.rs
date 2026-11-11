use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::browser::BrowserPanelState;
use crate::error::{Error, Result};
use crate::terminal::Terminal;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewMode {
    #[default]
    Edit,
    Preview,
    Split,
}

pub struct MarkdownEditor {
    pub text: String,
    pub file_path: Option<PathBuf>,
    pub dirty: bool,
    pub preview_mode: PreviewMode,
}

impl MarkdownEditor {
    /// Open a markdown file from disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read from disk.
    pub fn open(path: PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Editor(e.to_string()))?;
        Ok(Self {
            text,
            file_path: Some(path),
            dirty: false,
            preview_mode: PreviewMode::Preview,
        })
    }

    /// Create an empty scratch buffer.
    #[must_use]
    pub fn scratch() -> Self {
        Self {
            text: String::new(),
            file_path: None,
            dirty: false,
            preview_mode: PreviewMode::Edit,
        }
    }

    /// Save the buffer to its file path.
    ///
    /// # Errors
    ///
    /// Returns an error when the editor has no file path or the file write fails.
    pub fn save(&mut self) -> Result<()> {
        if let Some(path) = &self.file_path {
            std::fs::write(path, &self.text)?;
            self.dirty = false;
            Ok(())
        } else {
            Err(Error::Editor("no file path set".to_string()))
        }
    }

    /// Save only if dirty and a file path is set. Silently succeeds otherwise.
    pub fn save_if_dirty(&mut self) {
        if self.dirty
            && self.file_path.is_some()
            && let Err(e) = self.save()
        {
            tracing::warn!("failed to save editor buffer: {e}");
        }
    }
}

/// The content held inside a [`Panel`](crate::panel::Panel).
pub enum PanelContent {
    Terminal(Terminal),
    Editor(MarkdownEditor),
    Browser(Box<BrowserPanelState>),
    Device(crate::DevicePanelState),
    Block(crate::BlockPanelState),
}

impl PanelContent {
    #[must_use]
    pub fn device(&self) -> Option<&crate::DevicePanelState> {
        match self {
            Self::Device(device) => Some(device),
            Self::Terminal(_) | Self::Editor(_) | Self::Browser(_) | Self::Block(_) => None,
        }
    }

    #[must_use]
    pub fn terminal(&self) -> Option<&Terminal> {
        match self {
            Self::Terminal(t) => Some(t),
            Self::Editor(_) | Self::Browser(_) | Self::Device(_) | Self::Block(_) => None,
        }
    }

    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        match self {
            Self::Terminal(t) => Some(t),
            Self::Editor(_) | Self::Browser(_) | Self::Device(_) | Self::Block(_) => None,
        }
    }

    #[must_use]
    pub fn editor(&self) -> Option<&MarkdownEditor> {
        match self {
            Self::Editor(e) => Some(e),
            Self::Terminal(_) | Self::Browser(_) | Self::Device(_) | Self::Block(_) => None,
        }
    }

    pub fn editor_mut(&mut self) -> Option<&mut MarkdownEditor> {
        match self {
            Self::Editor(e) => Some(e),
            Self::Terminal(_) | Self::Browser(_) | Self::Device(_) | Self::Block(_) => None,
        }
    }

    #[must_use]
    pub fn browser(&self) -> Option<&BrowserPanelState> {
        match self {
            Self::Browser(b) => Some(b),
            Self::Terminal(_) | Self::Editor(_) | Self::Device(_) | Self::Block(_) => None,
        }
    }

    pub fn browser_mut(&mut self) -> Option<&mut BrowserPanelState> {
        match self {
            Self::Browser(b) => Some(b),
            Self::Terminal(_) | Self::Editor(_) | Self::Device(_) | Self::Block(_) => None,
        }
    }

    #[must_use]
    pub fn block(&self) -> Option<&crate::BlockPanelState> {
        match self {
            Self::Block(block) => Some(block),
            Self::Terminal(_) | Self::Editor(_) | Self::Browser(_) | Self::Device(_) => None,
        }
    }

    pub fn block_mut(&mut self) -> Option<&mut crate::BlockPanelState> {
        match self {
            Self::Block(block) => Some(block),
            Self::Terminal(_) | Self::Editor(_) | Self::Browser(_) | Self::Device(_) => None,
        }
    }
}
