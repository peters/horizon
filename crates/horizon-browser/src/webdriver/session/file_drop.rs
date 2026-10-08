use std::path::PathBuf;

use serde_json::json;

use super::Driver;
use crate::session::BrowserEventSender;
use crate::{BrowserControlFailure, semantic::check_script_error};

const PREPARE: &str = r"function(x, y, selector) {
    let doc = document, target;
    if (selector !== null) {
        target = doc.querySelector(selector);
        if (!target) throw new Error('The file drop target is no longer available');
        const style = getComputedStyle(target);
        if (style.display === 'none' || style.visibility === 'hidden' || target.matches(':disabled') || target.getAttribute('aria-disabled') === 'true') {
            throw new Error('The file drop target is not visible or enabled');
        }
        target.scrollIntoView({ block: 'center', inline: 'center' });
        const rect = target.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) throw new Error('The file drop target is not visible');
        x = rect.left + rect.width / 2; y = rect.top + rect.height / 2;
    } else {
    for (let depth = 0; depth < 16; depth++) {
        target = doc.elementFromPoint(x, y);
        if (!target) throw new Error('No file drop target at this position');
        while (target.shadowRoot) {
            const inner = target.shadowRoot.elementFromPoint(x, y);
            if (!inner || inner === target) break;
            target = inner;
        }
        if (target.tagName !== 'IFRAME') break;
        const next = target.contentDocument;
        if (!next) throw new Error('Cross-origin frame file drops are unsupported in Firefox');
        const rect = target.getBoundingClientRect();
        x -= rect.left + target.clientLeft; y -= rect.top + target.clientTop; doc = next;
    }
    }
    const input = document.createElement('input');
    input.type = 'file'; input.multiple = true; input.hidden = true;
    input._dropTarget = target; input._dropPoint = [x, y];
    document.documentElement.appendChild(input);
    return input;
}";

const DELIVER: &str = r"async function(input, count) {
    try {
    const target = input._dropTarget, point = input._dropPoint;

        if (!input.isConnected || !target?.isConnected) throw new Error('The original file drop target is no longer in the document');
        const files = Array.from(input.files || []);
        if (files.length !== count) throw new Error('The browser did not attach all dropped files');
        await Promise.all(files.map(file => file.size ? Promise.all([file.slice(0, 1).arrayBuffer(), file.slice(-1).arrayBuffer()]) : file.arrayBuffer()));
        if (!target.isConnected) throw new Error('The file drop target changed');
        const win = target.ownerDocument.defaultView, data = new win.DataTransfer();
        for (const file of files) data.items.add(file);
        const event = type => new win.DragEvent(type, { bubbles: true, cancelable: true, composed: true, clientX: point[0], clientY: point[1], dataTransfer: data });
        target.dispatchEvent(event('dragenter'));
        if (target.dispatchEvent(event('dragover'))) {
            target.dispatchEvent(event('dragleave'));
            throw new Error('This element does not accept file drops');
        }
        target.dispatchEvent(event('drop'));
        return JSON.stringify({ delivered: true });
    } catch (error) {
        return JSON.stringify({ error: { code: 'file_drop_failed', message: String(error.message || error) } });
    } finally { input.remove(); }
}";

impl Driver {
    pub(super) fn drop_files(
        &mut self,
        x: f64,
        y: f64,
        paths: &[PathBuf],
        events: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        self.drop_files_at(x, y, None, paths, events)
    }

    pub(super) fn drop_files_on_target(
        &mut self,
        selector: &str,
        paths: &[PathBuf],
        events: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        self.drop_files_at(0.0, 0.0, Some(selector), paths, events)
    }

    fn drop_files_at(
        &mut self,
        x: f64,
        y: f64,
        selector: Option<&str>,
        paths: &[PathBuf],
        events: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        if !self.firefox_bidi() {
            return Err(BrowserControlFailure::new(
                "unsupported_backend",
                "File drops require local Chromium or Firefox; use the file upload control on this backend",
            ));
        }
        crate::session::file_drop::validate_drop(x, y, paths)?;
        let generation = self.generation;
        let prepared = self
            .call_bidi(
                "script.callFunction",
                &json!({
                    "functionDeclaration": PREPARE, "awaitPromise": false,
                    "target": {"context": self.context_id}, "resultOwnership": "none",
                    "arguments": [{"type":"number","value":x},{"type":"number","value":y},
                        selector.map_or_else(|| json!({"type":"null"}), |value| json!({"type":"string","value":value}))]
                }),
                events,
            )
            .map_err(drop_failure)?;
        let element = prepared
            .pointer("/result/sharedId")
            .and_then(serde_json::Value::as_str)
            .map(|shared_id| json!({"sharedId":shared_id}))
            .ok_or_else(|| drop_failure("The original file drop target is no longer available"))?;
        let result = (|| {
            if generation != self.generation || self.retain_frame_during_navigation {
                return Err(drop_failure("The page changed before file selection"));
            }
            self.call_bidi(
                "input.setFiles",
                &json!({
                    "context":self.context_id,"element":element,"files":crate::file_chooser::wire_paths(paths)?
                }),
                events,
            )
            .map_err(drop_failure)?;
            if generation != self.generation || self.retain_frame_during_navigation {
                return Err(drop_failure("The page changed during file selection"));
            }
            let response = self.call_bidi("script.callFunction", &json!({
                "functionDeclaration":DELIVER,"awaitPromise":true,
                "target":{"context":self.context_id},"arguments":[element,{"type":"number","value":paths.len()}],
                "resultOwnership":"none"
            }), events).map_err(drop_failure)?;
            let text = response
                .pointer("/result/value")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| drop_failure("The original file drop document is no longer available"))?;
            let value = serde_json::from_str(text).map_err(|_| drop_failure("Invalid file drop result"))?;
            check_script_error(&value)?;
            if value.get("delivered").and_then(serde_json::Value::as_bool) != Some(true) {
                return Err(drop_failure("No completed file drop"));
            }
            Ok(())
        })();
        let _ = self.call_bidi(
            "script.callFunction",
            &json!({
                "functionDeclaration":"function(input) { input.remove(); }", "awaitPromise":false,
                "target":{"context":self.context_id},"arguments":[element],"resultOwnership":"none"
            }),
            events,
        );
        if !self.retain_frame_during_navigation {
            self.frames.demand();
        }
        result
    }
}

fn drop_failure(message: impl Into<String>) -> BrowserControlFailure {
    BrowserControlFailure::new("file_drop_failed", message.into())
}
