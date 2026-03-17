use std::collections::HashSet;
use std::sync::Arc;

use crate::git_status::GitStatus;

/// Panel-level state for the Git Changes viewer.
#[derive(Default)]
pub struct DiffViewer {
    pub status: Option<Arc<GitStatus>>,
    pub expanded_files: HashSet<String>,
}

impl DiffViewer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            status: None,
            expanded_files: HashSet::new(),
        }
    }

    /// Replace the current status snapshot.
    pub fn update(&mut self, status: Arc<GitStatus>) {
        self.status = Some(status);
    }

    /// Toggle inline diff expansion for a file path.
    pub fn toggle_file(&mut self, path: &str) {
        if !self.expanded_files.remove(path) {
            self.expanded_files.insert(path.to_string());
        }
    }

    #[must_use]
    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded_files.contains(path)
    }
}
