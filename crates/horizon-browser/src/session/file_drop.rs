use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use super::{BrowserEventSender, DriverState};
use crate::cdp::CdpLink;
use crate::frames::FrameSlot;
use crate::{BrowserControlAction, BrowserControlFailure, BrowserTarget};

pub(crate) fn validate_drop(x: f64, y: f64, paths: &[PathBuf]) -> Result<(), BrowserControlFailure> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err(BrowserControlFailure::new(
            "invalid_input",
            "File drop coordinates must be finite and nonnegative",
        ));
    }
    BrowserControlAction::DropFiles {
        target: BrowserTarget::Selector {
            selector: "html".into(),
        },
        paths: paths.to_vec(),
        sources: Vec::new(),
    }
    .validate()
    .map_err(|message| BrowserControlFailure::new("invalid_input", message))?;
    crate::semantic_files::local_file_facts(paths)?;
    Ok(())
}

impl DriverState {
    pub(super) fn dispatch_file_drop(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        x: f64,
        y: f64,
        paths: &[PathBuf],
    ) -> Result<bool, BrowserControlFailure> {
        let result = self.drop_files(link, events, slot, x, y, paths);
        if let Err(error) = &result {
            let _ = events.send(super::BrowserEvent::NavigationFailed(format!(
                "File drop failed: {}",
                error.message
            )));
        }
        result.map(|()| false)
    }

    pub(super) fn drop_files(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        x: f64,
        y: f64,
        paths: &[PathBuf],
    ) -> Result<(), BrowserControlFailure> {
        validate_drop(x, y, paths)?;
        let files = crate::file_chooser::wire_paths(paths)?;
        let generation = self.semantic.generation();
        let data = json!({"items": [], "files": files, "dragOperationsMask": 1});
        for kind in ["dragEnter", "dragOver", "drop"] {
            if self.semantic.generation() != generation || self.top_frame_navigating {
                return Err(BrowserControlFailure::new(
                    "drop_navigation_invalidated",
                    "The page changed during the file drop",
                ));
            }
            self.send_page_command(
                link,
                events,
                slot,
                "Input.dispatchDragEvent",
                &json!({"type": kind, "x": x, "y": y, "data": data}),
            )
            .map_err(|error| BrowserControlFailure::new("file_drop_failed", error.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_invalid_coordinates_directories_and_missing_files_before_dispatch() {
        let root = tempfile::tempdir().expect("root");
        let file = root.path().join("notes.txt");
        std::fs::write(&file, b"synthetic").expect("file");
        assert!(validate_drop(10.0, 20.0, std::slice::from_ref(&file)).is_ok());
        for (x, y) in [(f64::NAN, 1.0), (1.0, f64::INFINITY), (-1.0, 1.0)] {
            assert_eq!(
                validate_drop(x, y, std::slice::from_ref(&file)).unwrap_err().code,
                "invalid_input"
            );
        }
        assert_eq!(
            validate_drop(1.0, 1.0, &[root.path().into()]).unwrap_err().code,
            "not_a_file"
        );
        assert_eq!(
            validate_drop(1.0, 1.0, &[root.path().join("missing")])
                .unwrap_err()
                .code,
            "file_not_found"
        );
        assert_eq!(
            validate_drop(1.0, 1.0, &vec![file; 33]).unwrap_err().code,
            "invalid_input"
        );
    }
}
