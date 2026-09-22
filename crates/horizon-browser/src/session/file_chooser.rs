use std::sync::Arc;

use serde_json::json;

use crate::cdp::{CdpEvent, CdpLink};
use crate::file_chooser::{ChooserBinding, FileChooserAnswer, audit_choice, wire_paths};
use crate::frames::FrameSlot;
use crate::semantic_files::{
    FILE_INPUT_PROBE_FUNCTION, check_attachment_request, local_file_facts, parse_file_input_probe,
};
use crate::{BrowserAuditStatus, BrowserControlFailure};

use super::semantic::FileInputTarget;
use super::{BrowserEvent, BrowserEventSender, DriverState};

#[derive(Debug, Default)]
pub(super) struct ChooserState {
    incoming: Option<Incoming>,
    binding: ChooserBinding,
    session: Option<String>,
    target: Option<Target>,
}

#[derive(Debug)]
struct Incoming {
    node: u64,
    session: String,
    generation: u64,
    revision: u64,
}

#[derive(Debug)]
struct Target {
    request: u64,
    object: String,
    session: String,
    generation: u64,
    revision: u64,
}

impl DriverState {
    pub(super) fn file_chooser_session_live(&self, session: &str) -> bool {
        self.session_id.is_some()
            && (self.session_id.as_deref() == Some(session) || self.clipboard.iframe_sessions.contains(session))
    }

    pub(super) fn attach_file_chooser_iframe(
        &mut self,
        link: &mut CdpLink,
        events: &BrowserEventSender,
        slot: &Arc<FrameSlot>,
        event: &CdpEvent<'_>,
    ) {
        let Some(session) = super::clipboard::target_event_session_id(event.params, event.session_id) else {
            return;
        };
        for (method, params) in [
            ("Page.enable", json!({})),
            ("Page.setInterceptFileChooserDialog", json!({"enabled":true})),
        ] {
            if !self.file_chooser_session_live(session) {
                break;
            }
            if let Err(error) = self.call_and_ack(link, events, slot, method, &params, Some(session)) {
                tracing::warn!(target: "browser", "iframe file chooser setup failed: {error}");
                break;
            }
        }
    }

    pub(super) fn retire_file_chooser_session(&mut self, event: &CdpEvent<'_>, events: &BrowserEventSender) {
        let session = super::clipboard::target_event_session_id(event.params, event.session_id);
        if session.is_some() && session == self.file_chooser.session.as_deref() {
            self.invalidate_file_chooser_frame(None, events);
        }
    }

    pub(super) fn handle_file_chooser_event(&mut self, event: &CdpEvent<'_>, events: &BrowserEventSender) {
        if event.method == "Page.fileChooserOpened" {
            self.note_file_chooser(event);
        } else if event
            .session_id
            .is_some_and(|session| self.file_chooser_session_live(session))
            && let Some(frame) = event.params.get("frameId").and_then(serde_json::Value::as_str)
        {
            self.invalidate_file_chooser_frame(Some(frame), events);
        }
    }

    pub(super) fn note_file_chooser(&mut self, event: &CdpEvent<'_>) {
        if !event
            .session_id
            .is_some_and(|session| self.file_chooser_session_live(session))
        {
            return;
        }
        if let (Some(node), Some(session), Some(frame)) = (
            event.params.get("backendNodeId").and_then(serde_json::Value::as_u64),
            event.session_id,
            event.params.get("frameId").and_then(serde_json::Value::as_str),
        ) {
            self.config.frame_slot.file_chooser().invalidate();
            self.manifest_dirty = true;
            self.file_chooser.session = Some(session.to_string());
            let revision = self.file_chooser.binding.start(frame.to_string());
            self.file_chooser.incoming = Some(Incoming {
                node,
                session: session.to_string(),
                generation: self.semantic.generation(),
                revision,
            });
        }
    }

    pub(super) fn invalidate_file_chooser_frame(&mut self, frame: Option<&str>, events: &BrowserEventSender) {
        if self.file_chooser.binding.invalidate(frame) {
            self.file_chooser.incoming = None;
            self.config.frame_slot.file_chooser().invalidate();
            self.manifest_dirty = true;
            events.wake_ui();
        }
    }

    pub(super) fn tick_file_chooser(&mut self, link: &mut CdpLink, events: &BrowserEventSender, slot: &Arc<FrameSlot>) {
        if self.file_chooser.incoming.is_some()
            && let Some(target) = &self.file_chooser.target
        {
            slot.file_chooser().invalidate_request(target.request);
        }
        if let Some(target) = self.file_chooser.target.take() {
            let live = self.file_chooser.binding.current(target.revision)
                && target.generation == self.semantic.generation()
                && self.file_chooser_session_live(&target.session)
                && slot
                    .file_chooser()
                    .request()
                    .is_some_and(|request| request.id == target.request);
            if live {
                if let Some(answer) = slot.file_chooser().take_answer(target.request) {
                    if let FileChooserAnswer::Files(paths) = answer {
                        let result = self.apply_file_choice(link, events, slot, &target, &paths);
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
                    slot.file_chooser().invalidate_request(target.request);
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
        let Some(incoming) = self.file_chooser.incoming.take() else {
            return;
        };
        if incoming.generation != self.semantic.generation() || !self.file_chooser_session_live(&incoming.session) {
            return;
        }
        let result = self.open_file_choice(link, events, slot, incoming);
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
        incoming: Incoming,
    ) -> Result<(), BrowserControlFailure> {
        let Incoming {
            node,
            session,
            generation,
            revision,
        } = incoming;
        let session = session.as_str();
        let resolved = self
            .call_and_ack(
                link,
                events,
                slot,
                "DOM.resolveNode",
                &json!({"backendNodeId":node}),
                Some(session),
            )
            .map_err(|error| BrowserControlFailure::new("input_failed", error.to_string()))?;
        let object = resolved
            .pointer("/object/objectId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| BrowserControlFailure::new("input_failed", "The file input is no longer available"))?
            .to_string();
        let result = (|| {
            let value = self.file_input_value_in_session(
                link,
                events,
                slot,
                FileInputTarget {
                    object: &object,
                    session,
                },
                FILE_INPUT_PROBE_FUNCTION,
            )?;
            let probe = parse_file_input_probe(&value)?;
            if generation != self.semantic.generation() || !self.file_chooser_session_live(session) {
                return Err(BrowserControlFailure::new(
                    "input_failed",
                    "The page changed while opening the chooser",
                ));
            }
            let origin = self
                .file_input_value_in_session(
                    link,
                    events,
                    slot,
                    FileInputTarget {
                        object: &object,
                        session,
                    },
                    "element => element.ownerDocument.location.origin",
                )?
                .as_str()
                .unwrap_or("this page")
                .to_string();
            if !self.file_chooser.binding.current(revision)
                || generation != self.semantic.generation()
                || !self.file_chooser_session_live(session)
            {
                return Err(BrowserControlFailure::new("stale_target", "The frame changed"));
            }
            let request = slot.file_chooser().open(probe.multiple, probe.accept, origin);
            self.file_chooser.target = Some(Target {
                request,
                object: object.clone(),
                session: session.to_string(),
                generation,
                revision,
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
        target: &Target,
        paths: &[std::path::PathBuf],
    ) -> Result<(), BrowserControlFailure> {
        let files = wire_paths(paths)?;
        let _ = local_file_facts(paths)?;
        let value = self.file_input_value_in_session(
            link,
            events,
            slot,
            FileInputTarget {
                object: &target.object,
                session: &target.session,
            },
            FILE_INPUT_PROBE_FUNCTION,
        )?;
        check_attachment_request(&parse_file_input_probe(&value)?, paths)?;
        if !self.file_chooser.binding.current(target.revision)
            || target.generation != self.semantic.generation()
            || !self.file_chooser_session_live(&target.session)
        {
            return Err(BrowserControlFailure::new("stale_target", "The frame changed"));
        }
        self.call_and_ack(
            link,
            events,
            slot,
            "DOM.setFileInputFiles",
            &json!({"objectId":target.object,"files":files}),
            Some(&target.session),
        )
        .map_err(|error| BrowserControlFailure::new("input_failed", error.to_string()))?;
        // Native attachment dispatches change handlers, which may immediately
        // consume and clear the input. Never retry an already delivered choice.
        Ok(())
    }
}
