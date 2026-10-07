use std::path::{Path, PathBuf};

use super::{DirectoryEntry, DirectoryListing};
use crate::dir_search;

#[derive(Debug, thiserror::Error)]
pub enum FileSelectionError {
    #[error("Choose one file for this upload")]
    SingleFile,
    #[error("Select files, not folders")]
    NotFile,
    #[error("{0} does not match the accepted file types")]
    TypeMismatch(String),
    #[error("Select at most 32 files for one upload")]
    TooManyFiles,
}

/// File selection and path matching shared by host upload pickers.
pub struct FilePickerState {
    pub directory: PathBuf,
    pub selected: Vec<PathBuf>,
    pub show_hidden: bool,
    multiple: bool,
    accept: String,
}

impl FilePickerState {
    #[must_use]
    pub fn new(directory: PathBuf, multiple: bool, accept: String) -> Self {
        Self {
            directory,
            selected: Vec::new(),
            show_hidden: false,
            multiple,
            accept,
        }
    }

    /// A path query lists its parent; a trailing separator lists the directory.
    #[must_use]
    pub fn query_location(&self, query: &str) -> (PathBuf, String) {
        let query = query.trim();
        let Some(path) = dir_search::resolve_query_path(query) else {
            return (self.directory.clone(), query.to_lowercase());
        };
        if query.ends_with(std::path::is_separator) || query == "~" {
            return (path, String::new());
        }
        let parent = path.parent().unwrap_or(&path).to_path_buf();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        (parent, name)
    }

    #[must_use]
    pub fn visible_entries<'a>(&self, listing: &'a DirectoryListing, filter: &str) -> Vec<&'a DirectoryEntry> {
        listing
            .entries
            .iter()
            .filter(|entry| {
                let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
                (self.show_hidden || !name.starts_with('.') || filter.starts_with('.'))
                    && name.to_lowercase().contains(filter)
                    && (entry.directory || horizon_browser::accepts_file(&self.accept, &entry.path))
            })
            .collect()
    }

    /// Toggle one file while respecting the attachment count limit.
    /// # Errors
    /// Returns an error if the selection would contain more than 32 files.
    pub fn toggle(&mut self, path: &Path) -> Result<(), FileSelectionError> {
        if let Some(index) = self.selected.iter().position(|selected| selected == path) {
            self.selected.remove(index);
        } else {
            if self.multiple && self.selected.len() >= horizon_browser::MAX_ATTACHMENT_FILES {
                return Err(FileSelectionError::TooManyFiles);
            }
            if !self.multiple {
                self.selected.clear();
            }
            self.selected.push(path.to_path_buf());
        }
        Ok(())
    }

    /// Validate all dropped paths before changing the selection.
    /// # Errors
    /// Rejects folders, disallowed file types, and invalid selection counts.
    pub fn select_dropped(&mut self, paths: &[PathBuf]) -> Result<(), FileSelectionError> {
        if paths.is_empty() || (!self.multiple && paths.len() > 1) {
            return Err(FileSelectionError::SingleFile);
        }
        for path in paths {
            if !path.is_file() {
                return Err(FileSelectionError::NotFile);
            }
            if !horizon_browser::accepts_file(&self.accept, path) {
                return Err(FileSelectionError::TypeMismatch(
                    path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                ));
            }
        }
        let new_count = paths
            .iter()
            .filter(|path| !self.selected.contains(path))
            .collect::<std::collections::HashSet<_>>()
            .len();
        if self.multiple && self.selected.len() + new_count > horizon_browser::MAX_ATTACHMENT_FILES {
            return Err(FileSelectionError::TooManyFiles);
        }
        if !self.multiple {
            self.selected.clear();
        }
        for path in paths {
            if !self.selected.contains(path) {
                self.selected.push(path.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_queries_separate_directory_navigation_from_name_filtering() {
        let root = tempfile::tempdir().expect("directory");
        let state = FilePickerState::new(root.path().into(), true, String::new());
        let child = root.path().join("Reports");
        assert_eq!(
            state.query_location(&format!("{}/", child.display())),
            (child, String::new())
        );
        assert_eq!(
            state.query_location(&format!("{}/Report", root.path().display())),
            (root.path().into(), "report".into())
        );
        assert_eq!(state.query_location("invoice"), (root.path().into(), "invoice".into()));
    }

    #[test]
    fn selection_survives_navigation_and_single_file_selection_replaces() {
        let mut state = FilePickerState::new(PathBuf::new(), true, String::new());
        state.toggle(Path::new("/first/a.pdf")).expect("selection");
        state.directory = "/second".into();
        state.toggle(Path::new("/second/b.pdf")).expect("selection");
        assert_eq!(state.selected.len(), 2);
        state.toggle(Path::new("/first/a.pdf")).expect("selection");
        assert_eq!(state.selected, [PathBuf::from("/second/b.pdf")]);
        state.multiple = false;
        state.toggle(Path::new("/first/a.pdf")).expect("selection");
        assert_eq!(state.selected, [PathBuf::from("/first/a.pdf")]);
    }

    #[test]
    fn listing_filters_names_hidden_files_and_types_but_keeps_folders() {
        let state = FilePickerState::new(PathBuf::new(), true, ".pdf".into());
        let listing = DirectoryListing {
            entries: [
                ("Reports", true),
                ("Report.PDF", false),
                ("photo.png", false),
                (".private.pdf", false),
            ]
            .into_iter()
            .map(|(name, directory)| DirectoryEntry {
                path: name.into(),
                directory,
                size: 0,
            })
            .collect(),
            truncated: false,
        };
        assert_eq!(state.visible_entries(&listing, "report").len(), 2);
        assert_eq!(state.visible_entries(&listing, "").len(), 2);
        assert_eq!(state.visible_entries(&listing, ".private").len(), 1);
    }

    #[test]
    fn invalid_multi_file_drop_preserves_the_previous_selection() {
        let root = tempfile::tempdir().expect("directory");
        let valid = root.path().join("report.pdf");
        let invalid = root.path().join("photo.png");
        std::fs::write(&valid, "synthetic").expect("file");
        std::fs::write(&invalid, "synthetic").expect("file");
        let mut state = FilePickerState::new(root.path().into(), true, ".pdf".into());
        state.select_dropped(std::slice::from_ref(&valid)).expect("selection");
        assert!(state.select_dropped(&[valid.clone(), invalid]).is_err());
        assert_eq!(state.selected, [valid]);
    }
}
