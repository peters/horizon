use std::sync::Arc;

use serde_json::json;

use crate::cdp::{CdpEvent, CdpLink};
use crate::file_chooser::{FileChooserAnswer, audit_choice};
use crate::frames::FrameSlot;
use crate::semantic_files::{
    FILE_INPUT_PROBE_FUNCTION, check_attachment_request, local_file_facts, parse_file_input_probe,
};
use crate::{BrowserAuditStatus, BrowserControlFailure};

use super::{BrowserEvent, BrowserEventSender, DriverState};

#[derive(Debug, Default)]
pub(super) struct ChooserState {
    incoming: Option<(u64, String, u64)>,
    target: Option<Target>,
}

#[derive(Debug)]
struct Target {
    request: u64,
    object: String,
    session: String,
    generation: u64,
}

impl DriverState {
    pub(super) fn note_file_chooser(&mut self, event: &CdpEvent<'_>) {
        if event.session_id != self.session_id.as_deref() {
            return;
        }
        if let (Some(node), Some(session)) = (
            event.params.get("backendNodeId").and_then(serde_json::Value::as_u64),
            event.session_id,
        ) {
            self.config.frame_slot.file_chooser().invalidate();
            self.file_chooser.incoming = Some((node, session.to_string(), self.semantic.generation()));
        }
    }

    pub(super) fn tick_file_chooser(&mut self, link: &mut CdpLink, events: &BrowserEventSender, slot: &Arc<FrameSlot>) {
        if self.file_chooser.incoming.is_some()
            && let Some(target) = &self.file_chooser.target
        {
            slot.file_chooser().invalidate_request(target.request);
        }
        if let Some(target) = self.file_chooser.target.take() {
            let live = target.generation == self.semantic.generation()
                && self.session_id.as_deref() == Some(&target.session)
                && slot
                    .file_chooser()
                    .request()
                    .is_some_and(|request| request.id == target.request);
            if live {
                if let Some(answer) = slot.file_chooser().take_answer(target.request) {
                    if let FileChooserAnswer::Files(paths) = answer {
                        let result = self.apply_file_choice(link, events, slot, &target.object, &paths);
                        audit_choice(
                            &self.config,
                            &paths,
                            if result.is_ok() {
                                BrowserAuditStatus::Completed
                            } else {
                                BrowserAuditStatus::Failed
                            },
                        );
                        if let Err(error) = result {
                            slot.file_chooser().retry(target.request, error.message);
                            self.file_chooser.target = Some(target);
                            events.wake_ui();
                            return;
                        }
                    }
                    slot.file_chooser().invalidate();
                    self.manifest_dirty = true;
                    events.wake_ui();
                } else {
                    self.file_chooser.target = Some(target);
                    return;
                }
            }
            slot.file_chooser().invalidate_request(target.request);
            self.manifest_dirty = true;
            events.wake_ui();
            let _ = self.call_and_ack(
                link,
                events,
                slot,
                "Runtime.releaseObject",
                &json!({"objectId": target.object}),
                Some(&target.session),
            );
        }
        let Some((node, session, generation)) = self.file_chooser.incoming.take() else {
            return;
        };
        if generation != self.semantic.generation() || self.session_id.as_deref() != Some(&session) {
            return;
        }
        let result = self.open_file_choice(link, events, slot, node, &session, generation);
        if let Err(error) = result {
            let _ = events.send(BrowserEvent::NavigationFailed(format!(
                "Cannot open file selection: {}. Try the upload control again.",
                error.message
            )));
        }
    }

    fn open_file_choice(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        node: u64,
        session: &str,
        generation: u64,
    ) -> Result<(), BrowserControlFailure> {
        let resolved = self
            .send_page_command(link, events, slot, "DOM.resolveNode", &json!({"backendNodeId":node}))
            .map_err(|error| BrowserControlFailure::new("input_failed", error.to_string()))?;
        let object = resolved
            .pointer("/object/objectId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrowserControlFailure::new("input_failed", "The file input is no longer available"))?
            .to_string();
        let result = (|| {
            let value = self.file_input_value(link, events, slot, &object, FILE_INPUT_PROBE_FUNCTION)?;
            let probe = parse_file_input_probe(&value)?;
            if generation != self.semantic.generation() || self.session_id.as_deref() != Some(session) {
                return Err(BrowserControlFailure::new(
                    "input_failed",
                    "The page changed while opening the chooser",
                ));
            }
            let origin = self
                .file_input_value(
                    link,
                    events,
                    slot,
                    &object,
                    "element => element.ownerDocument.location.origin",
                )?
                .as_str()
                .unwrap_or("this page")
                .to_string();
            let request = slot.file_chooser().open(probe.multiple, probe.accept, origin);
            self.file_chooser.target = Some(Target {
                request,
                object: object.clone(),
                session: session.to_string(),
                generation,
            });
            self.manifest_dirty = true;
            events.wake_ui();
            Ok(())
        })();
        if result.is_err() {
            let _ = self.call_and_ack(
                link,
                events,
                slot,
                "Runtime.releaseObject",
                &json!({"objectId":object}),
                Some(session),
            );
        }
        result
    }

    fn apply_file_choice(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        object: &str,
        paths: &[std::path::PathBuf],
    ) -> Result<(), BrowserControlFailure> {
        if paths.is_empty() {
            return Err(BrowserControlFailure::new("invalid_input", "No files were selected"));
        }
        let _ = local_file_facts(paths)?;
        let value = self.file_input_value(link, events, slot, object, FILE_INPUT_PROBE_FUNCTION)?;
        check_attachment_request(&parse_file_input_probe(&value)?, paths)?;
        self.send_page_command(
            link,
            events,
            slot,
            "DOM.setFileInputFiles",
            &json!({"objectId":object,"files":paths}),
        )
        .map_err(|error| BrowserControlFailure::new("input_failed", error.to_string()))?;
        // Native attachment dispatches change handlers, which may immediately
        // consume and clear the input. Never retry an already delivered choice.
        Ok(())
    }
}
