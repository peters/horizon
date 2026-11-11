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
    /// Whether the text of this buffer waits for its write to its file.
    pub saving: bool,
    /// Why the file of this buffer could not be read or written, until the next
    /// attempt.
    pub error: Option<String>,
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
            saving: false,
            error: None,
        })
    }

    /// An editor of `path` whose text a worker read, so the frame opens no
    /// file.
    #[must_use]
    pub fn from_file(path: PathBuf, text: String) -> Self {
        Self {
            text,
            file_path: Some(path),
            dirty: false,
            preview_mode: PreviewMode::Preview,
            saving: false,
            error: None,
        }
    }

    /// Create an empty scratch buffer.
    #[must_use]
    pub fn scratch() -> Self {
        Self {
            text: String::new(),
            file_path: None,
            dirty: false,
            preview_mode: PreviewMode::Edit,
            saving: false,
            error: None,
        }
    }

    /// The empty buffer of `path`, whose text a worker reads. The panel of a
    /// file opens at once and shows the text when the read answers.
    #[must_use]
    pub fn waiting_for(path: PathBuf) -> Self {
        Self {
            text: String::new(),
            file_path: Some(path),
            dirty: false,
            preview_mode: PreviewMode::Preview,
            saving: false,
            error: None,
        }
    }

    /// Asks for the write of this buffer to its file. The frame of the editor
    /// queues it on the coordination worker and clears `saving` and `dirty` with
    /// the answer, so no frame writes the file.
    pub fn request_save(&mut self) {
        if !self.dirty || self.file_path.is_none() || self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
    }

    /// The write that this buffer still owes its file, with the text that it
    /// shows, for a panel that leaves the board. `None` when the file already
    /// holds the text of the buffer.
    ///
    /// The panel that leaves keeps nothing to clear, so the mark of a waiting
    /// write goes with the text: the write that the caller queues runs after the
    /// writes asked for before it, and the last one holds the newest text.
    #[must_use]
    pub fn take_pending_write(&mut self, panel_id: crate::PanelId) -> Option<crate::EditorSave> {
        let path = self.file_path.clone()?;
        if !self.dirty && !self.saving {
            return None;
        }
        self.dirty = false;
        self.saving = false;
        Some(crate::EditorSave {
            panel_id,
            path,
            text: self.text.clone(),
        })
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
