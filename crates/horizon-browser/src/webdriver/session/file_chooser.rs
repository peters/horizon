use serde_json::{Value, json};

use crate::file_chooser::{
    ChooserBinding, FileChooserAnswer, audit_choice, manual_readback_function, manual_readback_result, wire_paths,
};
use crate::semantic_files::{
    FILE_INPUT_PROBE_FUNCTION, check_attachment_request, local_file_facts, parse_file_input_probe,
};
use crate::session::{BrowserEvent, BrowserEventSender};
use crate::{BrowserAuditStatus, BrowserControlFailure};

use super::Driver;

#[derive(Debug, Default)]
pub(super) struct ChooserState {
    incoming: Option<Target>,
    binding: ChooserBinding,
    target: Option<Target>,
}

impl ChooserState {
    pub(super) fn retain_after_viewport_change(&mut self, generation: u64) {
        // Viewport changes invalidate semantic refs, but retain the original
        // document-bound file input and any pending user selection.
        for target in self.incoming.iter_mut().chain(self.target.iter_mut()) {
            target.generation = generation;
        }
    }
}

#[derive(Debug)]
struct Target {
    request: u64,
    element: Value,
    context: String,
    generation: u64,
    revision: u64,
}

impl Driver {
    pub(super) fn handle_file_chooser_event(&mut self, event: &Value, event_tx: &BrowserEventSender) -> bool {
        let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if matches!(
            method,
            "browsingContext.navigationStarted"
                | "browsingContext.fragmentNavigated"
                | "browsingContext.contextDestroyed"
        ) && let Some(context) = params.get("context").and_then(Value::as_str)
        {
            self.invalidate_file_chooser_context(context, event_tx);
        }

        if event.get("method").and_then(Value::as_str) != Some("input.fileDialogOpened") {
            return false;
        }
        if let Some(params) = event.get("params") {
            self.note_file_chooser(params);
        }
        true
    }

    pub(super) fn enable_file_chooser(&mut self, events: &BrowserEventSender) {
        if !self.firefox_bidi() {
            return;
        }
        let result = self.call_bidi(
            "session.subscribe",
            &json!({"events":["input.fileDialogOpened"],"contexts":[self.context_id]}),
            events,
        );
        match result {
            Ok(_) => {
                self.panel_slot.file_chooser().enable();
                self.coordination_dirty = true;
            }
            Err(error) => tracing::warn!(target: "browser", "manual file chooser unavailable: {error}"),
        }
    }

    pub(super) fn note_file_chooser(&mut self, params: &Value) {
        let Some(context) = params.get("context").and_then(Value::as_str) else {
            return;
        };
        let Some(element) = params
            .get("element")
            .filter(|element| element.get("sharedId").and_then(Value::as_str).is_some())
        else {
            return;
        };
        self.panel_slot.file_chooser().invalidate();
        self.coordination_dirty = true;
        let revision = self.file_chooser.binding.start(context.to_string());
        self.file_chooser.incoming = Some(Target {
            request: 0,
            element: element.clone(),
            context: context.to_string(),
            generation: self.generation,
            revision,
        });
    }

    pub(super) fn invalidate_file_chooser_context(&mut self, context: &str, events: &BrowserEventSender) {
        if self.file_chooser.binding.invalidate(Some(context)) {
            self.file_chooser.incoming = None;
            self.panel_slot.file_chooser().invalidate();
            self.coordination_dirty = true;
            events.wake_ui();
        }
    }

    pub(super) fn tick_file_chooser(&mut self, events: &BrowserEventSender) {
        if self.file_chooser.incoming.is_some()
            && let Some(target) = &self.file_chooser.target
        {
            self.panel_slot.file_chooser().invalidate_request(target.request);
        }
        if let Some(target) = self.file_chooser.target.take() {
            let live = self.file_chooser.binding.current(target.revision)
                && target.generation == self.generation
                && self
                    .panel_slot
                    .file_chooser()
                    .request()
                    .is_some_and(|request| request.id == target.request);
            if live {
                if let Some(answer) = self.panel_slot.file_chooser().take_answer(target.request) {
                    if let FileChooserAnswer::Files(paths) = answer {
                        let result = self.apply_file_choice(&target, &paths, events);
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
                            self.panel_slot.file_chooser().retry(target.request, error.message);
                            self.file_chooser.target = Some(target);
                            events.wake_ui();
                            return;
                        }
                    }
                    self.panel_slot.file_chooser().invalidate_request(target.request);
                    self.write_coordination(true);
                    events.wake_ui();
                } else {
                    self.file_chooser.target = Some(target);
                    return;
                }
            }
            self.panel_slot.file_chooser().invalidate_request(target.request);
            self.write_coordination(true);
            events.wake_ui();
        }
        let Some(mut target) = self.file_chooser.incoming.take() else {
            return;
        };
        if target.generation != self.generation || !self.file_chooser.binding.current(target.revision) {
            return;
        }
        let result = (|| {
            let value = self.chooser_value(&target, FILE_INPUT_PROBE_FUNCTION, events)?;
            let probe = parse_file_input_probe(&value)?;
            let origin = self.chooser_value(&target, "element => element.ownerDocument.location.origin", events)?;
            if target.generation != self.generation || !self.file_chooser.binding.current(target.revision) {
                return Err(BrowserControlFailure::new("stale_target", "The page changed"));
            }
            target.request = self.panel_slot.file_chooser().open(
                probe.multiple,
                probe.accept,
                origin.as_str().unwrap_or("this page").to_string(),
            );
            self.file_chooser.target = Some(target);
            self.write_coordination(true);
            events.wake_ui();
            Ok(())
        })();
        if let Err(error) = result {
            let _ = events.send(BrowserEvent::NavigationFailed(format!(
                "Cannot open file selection: {}. Try the upload control again.",
                error.message
            )));
        }
    }

    fn chooser_value(
        &mut self,
        target: &Target,
        function: &str,
        events: &BrowserEventSender,
    ) -> Result<Value, BrowserControlFailure> {
        let result = self.call_bidi("script.callFunction", &json!({
            "functionDeclaration":format!("async function(element) {{ return JSON.stringify(await ({function})(element)); }}"),
            "awaitPromise":true,"target":{"context":target.context},"arguments":[target.element],"resultOwnership":"none"
        }), events).map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        let text = result
            .pointer("/result/value")
            .and_then(Value::as_str)
            .ok_or_else(|| BrowserControlFailure::new("input_failed", "The file input is no longer available"))?;
        serde_json::from_str(text).map_err(|error| BrowserControlFailure::new("invalid_result", error.to_string()))
    }

    fn apply_file_choice(
        &mut self,
        target: &Target,
        paths: &[std::path::PathBuf],
        events: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        let files = wire_paths(paths)?;
        let expected = local_file_facts(paths)?;
        let probe = self.chooser_value(target, FILE_INPUT_PROBE_FUNCTION, events)?;
        check_attachment_request(&parse_file_input_probe(&probe)?, paths)?;
        if target.generation != self.generation || !self.file_chooser.binding.current(target.revision) {
            return Err(BrowserControlFailure::new("stale_target", "The page changed"));
        }
        self.call_bidi(
            "input.setFiles",
            &json!({"context":target.context,"element":target.element,"files":files}),
            events,
        )
        .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        manual_readback_result(self.chooser_value(target, &manual_readback_function(&expected), events))
    }
}
