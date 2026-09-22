//! Host-owned manual file choices. Page code never receives host paths until
//! the user confirms a live request bound to one browser input.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChooserStatus {
    #[default]
    Unsupported,
    Available,
    Pending,
}

impl FileChooserStatus {
    #[must_use]
    pub fn supported(self) -> bool {
        self != Self::Unsupported
    }
    #[must_use]
    pub fn pending(self) -> bool {
        self == Self::Pending
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChooserRequest {
    pub id: u64,
    pub multiple: bool,
    pub accept: String,
    pub origin: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChooserAnswer {
    Cancel,
    Files(Vec<PathBuf>),
}

#[derive(Debug, Default)]
struct State {
    supported: bool,
    answered: bool,
    error: Option<String>,
    sequence: u64,
    request: Option<Arc<FileChooserRequest>>,
    answer: Option<FileChooserAnswer>,
}

#[derive(Clone, Debug, Default)]
pub struct FileChooserHandle(Arc<Mutex<State>>);

impl FileChooserHandle {
    #[must_use]
    pub fn status(&self) -> FileChooserStatus {
        let state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.request.is_some() {
            FileChooserStatus::Pending
        } else if state.supported {
            FileChooserStatus::Available
        } else {
            FileChooserStatus::Unsupported
        }
    }

    pub(crate) fn blocks(&self, action: &crate::BrowserControlAction) -> bool {
        self.request().is_some()
            && matches!(
                action,
                crate::BrowserControlAction::Input { .. }
                    | crate::BrowserControlAction::Click { .. }
                    | crate::BrowserControlAction::Fill { .. }
                    | crate::BrowserControlAction::Scroll { .. }
                    | crate::BrowserControlAction::SetFiles { .. }
                    | crate::BrowserControlAction::Evaluate { .. }
            )
    }

    #[must_use]
    pub fn supported(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .supported
    }

    #[must_use]
    pub fn request(&self) -> Option<Arc<FileChooserRequest>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .request
            .clone()
    }

    /// Returns false when navigation or a newer chooser invalidated the answer.
    #[must_use]
    pub fn respond(&self, id: u64, answer: FileChooserAnswer) -> bool {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.request.as_ref().is_none_or(|request| request.id != id) || state.answered {
            return false;
        }
        state.answer = Some(answer);
        state.answered = true;
        true
    }

    pub(crate) fn enable(&self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .supported = true;
    }

    pub(crate) fn open(&self, multiple: bool, accept: String, origin: String) -> u64 {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sequence = state.sequence.wrapping_add(1);
        let id = state.sequence;
        state.request = Some(Arc::new(FileChooserRequest {
            id,
            multiple,
            accept,
            origin,
        }));
        state.answer = None;
        state.answered = false;
        state.error = None;
        id
    }

    pub(crate) fn take_answer(&self, id: u64) -> Option<FileChooserAnswer> {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.request.as_ref().is_some_and(|request| request.id == id) {
            state.answer.take()
        } else {
            None
        }
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.request = None;
        state.answer = None;
    }

    pub(crate) fn invalidate_request(&self, id: u64) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.request.as_ref().is_some_and(|request| request.id == id) {
            state.request = None;
            state.answer = None;
            state.error = None;
        }
    }

    #[must_use]
    pub fn take_error(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .error
            .take()
    }

    pub(crate) fn retry(&self, id: u64, message: String) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.request.as_ref().is_some_and(|request| request.id == id) {
            state.answer = None;
            state.answered = false;
            state.error = Some(message);
        }
    }

    pub(crate) fn reset(&self) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.request = None;
        state.answer = None;
        state.error = None;
        state.answered = false;
        state.supported = false;
    }
}

/// A lifecycle token remains visible while a protocol call temporarily owns
/// the target, so reentrant navigation cannot revive a stale dialog.
#[derive(Debug, Default)]
pub(crate) struct ChooserBinding {
    context: Option<String>,
    revision: u64,
}

impl ChooserBinding {
    pub(crate) fn start(&mut self, context: String) -> u64 {
        self.context = Some(context);
        self.revision = self.revision.wrapping_add(1);
        self.revision
    }

    pub(crate) fn current(&self, revision: u64) -> bool {
        self.context.is_some() && self.revision == revision
    }

    pub(crate) fn invalidate(&mut self, context: Option<&str>) -> bool {
        if self.context.is_none() || context.is_some_and(|context| self.context.as_deref() != Some(context)) {
            return false;
        }
        self.context = None;
        self.revision = self.revision.wrapping_add(1);
        true
    }
}

pub(crate) fn wire_paths(paths: &[PathBuf]) -> Result<Vec<&str>, crate::BrowserControlFailure> {
    if paths.is_empty() {
        return Err(crate::BrowserControlFailure::new(
            "invalid_input",
            "No files were selected",
        ));
    }
    paths
        .iter()
        .map(|path| {
            path.to_str().ok_or_else(|| {
                crate::BrowserControlFailure::new(
                    "invalid_input",
                    "The browser requires UTF-8 file paths; rename the file or its directory and try again",
                )
            })
        })
        .collect()
}

pub(crate) fn audit_choice(config: &crate::BrowserSessionConfig, paths: &[PathBuf], status: crate::BrowserAuditStatus) {
    if let Some(coordination) = &config.coordination {
        let entry = crate::BrowserAuditEntry::new(
            crate::new_action_id(),
            crate::BrowserAuditActor::User,
            status,
            crate::BrowserAuditAction::SetFiles {
                target: "manual file chooser".into(),
                paths: paths.iter().map(|path| path.to_string_lossy().into_owned()).collect(),
            },
        );
        if let Err(error) = coordination.record_action(&config.panel_local_id, &entry) {
            tracing::warn!(target: "browser", "manual file choice audit failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_invalidates_in_flight_choices_without_cancelling_sibling_frames() {
        let mut binding = ChooserBinding::default();
        let original = binding.start("child".into());
        assert!(!binding.invalidate(Some("sibling")));
        assert!(binding.current(original));
        assert!(binding.invalidate(Some("child")));
        assert!(!binding.current(original));
        let replacement = binding.start("child".into());
        assert!(!binding.current(original));
        assert!(binding.current(replacement));
        assert!(binding.invalidate(None));
        assert!(!binding.current(replacement));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_manual_choices_return_an_error_before_protocol_serialization() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/file-\xff.txt".to_vec()));
        assert_eq!(wire_paths(&[invalid]).unwrap_err().code, "invalid_input");
        assert_eq!(wire_paths(&[]).unwrap_err().code, "invalid_input");
        assert_eq!(
            wire_paths(&[PathBuf::from("/tmp/valid.txt")]).unwrap(),
            ["/tmp/valid.txt"]
        );
    }

    #[test]
    fn replaced_and_cancelled_requests_cannot_receive_old_choices() {
        let handle = FileChooserHandle::default();
        handle.enable();
        let first = handle.open(false, String::new(), "https://first.test".into());
        let second = handle.open(true, ".txt".into(), "https://second.test".into());
        assert!(!handle.respond(first, FileChooserAnswer::Files(vec![PathBuf::from("/private/a.txt")])));
        assert!(handle.respond(second, FileChooserAnswer::Cancel));
        assert!(!handle.respond(second, FileChooserAnswer::Files(vec![PathBuf::from("/private/a.txt")])));
        assert_eq!(handle.take_answer(second), Some(FileChooserAnswer::Cancel));
        assert!(!handle.respond(second, FileChooserAnswer::Cancel));
        handle.invalidate();
        assert!(!handle.respond(second, FileChooserAnswer::Cancel));
        assert!(handle.supported());
        handle.reset();
        assert!(!handle.supported());
    }

    #[test]
    fn retiring_an_old_target_preserves_its_replacement_and_blocks_only_page_changes() {
        let handle = FileChooserHandle::default();
        let first = handle.open(false, String::new(), "https://first.test".into());
        let second = handle.open(false, String::new(), "https://second.test".into());
        handle.invalidate_request(first);
        assert_eq!(handle.request().map(|request| request.id), Some(second));
        assert!(handle.blocks(&crate::BrowserControlAction::Evaluate {
            expression: "document.body.remove()".into()
        }));
        assert!(!handle.blocks(&crate::BrowserControlAction::Query {
            selector: "input".into(),
            max_results: 1
        }));
        handle.invalidate_request(second);
        assert!(!handle.blocks(&crate::BrowserControlAction::Evaluate {
            expression: "document.title".into()
        }));
        assert!(!handle.respond(second, FileChooserAnswer::Cancel));
    }

    #[test]
    fn failed_choice_can_be_corrected_once_and_old_errors_cannot_reopen_a_new_request() {
        let handle = FileChooserHandle::default();
        let first = handle.open(false, ".txt".into(), "https://files.test".into());
        assert!(handle.respond(first, FileChooserAnswer::Files(vec!["/missing.txt".into()])));
        let _ = handle.take_answer(first);
        handle.retry(first, "File was removed".into());
        assert_eq!(handle.take_error().as_deref(), Some("File was removed"));
        assert!(handle.respond(first, FileChooserAnswer::Cancel));
        let second = handle.open(false, String::new(), "https://files.test".into());
        handle.retry(first, "Late error".into());
        assert!(handle.take_error().is_none());
        assert!(handle.respond(second, FileChooserAnswer::Cancel));
    }
}
