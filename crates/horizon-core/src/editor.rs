use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::browser::BrowserPanelState;
use crate::error::{Error, Result};
use crate::git_changes::DiffViewer;
use crate::terminal::Terminal;
use crate::usage_dashboard::UsageDashboard;

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
    /// Character offset for dictation insert, updated from the edit widget.
    pub caret: usize,
    /// When set, the edit widget must push `caret` into egui before reading
    /// the widget's cursor back; otherwise a focused `TextEdit` restores the
    /// pre-dictation range and the next insert lands before the last one.
    pending_caret: bool,
}

impl MarkdownEditor {
    /// Open a markdown file from disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read from disk.
    pub fn open(path: PathBuf) -> Result<Self> {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Editor(e.to_string()))?;
        let caret = text.chars().count();
        Ok(Self {
            text,
            file_path: Some(path),
            dirty: false,
            preview_mode: PreviewMode::Preview,
            caret,
            pending_caret: false,
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
            caret: 0,
            pending_caret: false,
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

    /// Insert dictated text at the caret. Non-edit modes switch to edit so
    /// the inserted text is visible (Split is preview-only in the widget).
    pub fn insert_dictation(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.preview_mode != PreviewMode::Edit {
            self.preview_mode = PreviewMode::Edit;
        }
        let at = byte_index_for_char(&self.text, self.caret);
        self.text.insert_str(at, text);
        self.caret += text.chars().count();
        self.pending_caret = true;
        self.dirty = true;
    }

    /// Push a programmatic caret into the next edit-widget frame.
    pub fn set_caret(&mut self, caret: usize) {
        self.caret = caret.min(self.text.chars().count());
        self.pending_caret = true;
    }

    /// Take a dictation caret that the edit widget must apply before show.
    #[must_use]
    pub fn take_pending_caret(&mut self) -> Option<usize> {
        if !self.pending_caret {
            return None;
        }
        self.pending_caret = false;
        Some(self.caret)
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

fn byte_index_for_char(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(offset, _)| offset)
}

/// The content held inside a [`Panel`](crate::panel::Panel).
pub enum PanelContent {
    Terminal(Terminal),
    Editor(MarkdownEditor),
    GitChanges(DiffViewer),
    Usage(UsageDashboard),
    Browser(Box<BrowserPanelState>),
}

impl PanelContent {
    #[must_use]
    pub fn terminal(&self) -> Option<&Terminal> {
        match self {
            Self::Terminal(t) => Some(t),
            Self::Editor(_) | Self::GitChanges(_) | Self::Usage(_) | Self::Browser(_) => None,
        }
    }

    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        match self {
            Self::Terminal(t) => Some(t),
            Self::Editor(_) | Self::GitChanges(_) | Self::Usage(_) | Self::Browser(_) => None,
        }
    }

    #[must_use]
    pub fn editor(&self) -> Option<&MarkdownEditor> {
        match self {
            Self::Editor(e) => Some(e),
            Self::Terminal(_) | Self::GitChanges(_) | Self::Usage(_) | Self::Browser(_) => None,
        }
    }

    pub fn editor_mut(&mut self) -> Option<&mut MarkdownEditor> {
        match self {
            Self::Editor(e) => Some(e),
            Self::Terminal(_) | Self::GitChanges(_) | Self::Usage(_) | Self::Browser(_) => None,
        }
    }

    #[must_use]
    pub fn git_changes(&self) -> Option<&DiffViewer> {
        match self {
            Self::GitChanges(v) => Some(v),
            Self::Terminal(_) | Self::Editor(_) | Self::Usage(_) | Self::Browser(_) => None,
        }
    }

    pub fn git_changes_mut(&mut self) -> Option<&mut DiffViewer> {
        match self {
            Self::GitChanges(v) => Some(v),
            Self::Terminal(_) | Self::Editor(_) | Self::Usage(_) | Self::Browser(_) => None,
        }
    }

    #[must_use]
    pub fn usage(&self) -> Option<&UsageDashboard> {
        match self {
            Self::Usage(u) => Some(u),
            Self::Terminal(_) | Self::Editor(_) | Self::GitChanges(_) | Self::Browser(_) => None,
        }
    }

    pub fn usage_mut(&mut self) -> Option<&mut UsageDashboard> {
        match self {
            Self::Usage(u) => Some(u),
            Self::Terminal(_) | Self::Editor(_) | Self::GitChanges(_) | Self::Browser(_) => None,
        }
    }

    #[must_use]
    pub fn browser(&self) -> Option<&BrowserPanelState> {
        match self {
            Self::Browser(b) => Some(b),
            Self::Terminal(_) | Self::Editor(_) | Self::GitChanges(_) | Self::Usage(_) => None,
        }
    }

    pub fn browser_mut(&mut self) -> Option<&mut BrowserPanelState> {
        match self {
            Self::Browser(b) => Some(b),
            Self::Terminal(_) | Self::Editor(_) | Self::GitChanges(_) | Self::Usage(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MarkdownEditor, PreviewMode};

    #[test]
    fn dictation_inserts_at_the_caret_and_leaves_preview() {
        let mut editor = MarkdownEditor {
            text: "ab cd".to_owned(),
            file_path: None,
            dirty: false,
            preview_mode: PreviewMode::Preview,
            caret: 2,
            pending_caret: false,
        };
        editor.insert_dictation("XY ");
        assert_eq!(editor.text, "abXY  cd");
        assert_eq!(editor.caret, 5);
        assert_eq!(editor.take_pending_caret(), Some(5));
        assert!(editor.take_pending_caret().is_none());
        assert!(editor.dirty);
        assert_eq!(editor.preview_mode, PreviewMode::Edit);
    }

    #[test]
    fn dictation_switches_split_preview_to_edit() {
        let mut editor = MarkdownEditor {
            text: "ab cd".to_owned(),
            file_path: None,
            dirty: false,
            preview_mode: PreviewMode::Split,
            caret: 2,
            pending_caret: false,
        };
        editor.insert_dictation("XY ");
        assert_eq!(editor.preview_mode, PreviewMode::Edit);
        assert_eq!(editor.text, "abXY  cd");
    }

    #[test]
    fn consecutive_dictation_advances_the_pending_caret() {
        let mut editor = MarkdownEditor::scratch();
        editor.insert_dictation("alpha ");
        editor.insert_dictation("bravo ");
        assert_eq!(editor.text, "alpha bravo ");
        assert_eq!(editor.take_pending_caret(), Some(12));
    }
}
