//! Chromium host overlay for native size-1 `<select>` popups.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::cdp::{CdpErrorInfo, CdpLink};
use crate::frames::FrameSlot;
use crate::input::BrowserInputCdpExt;
use crate::native_select::{
    NativeSelectApply, NativeSelectProbe, apply_expression, parse_apply, parse_probe, probe_expression,
};
use crate::{BrowserInput, BrowserKey, BrowserModifiers};

use super::{BrowserEventSender, DriverState};

#[derive(Debug, Default)]
pub(super) struct NativeSelectState {
    probe_id: Option<u64>,
    in_flight: Option<NativeSelectProbe>,
    pending: Option<NativeSelectProbe>,
    last_popup_focused: bool,
}

impl DriverState {
    pub(super) fn dismiss_native_select(&mut self, event_tx: &BrowserEventSender) {
        self.native_select.pending = None;
        self.native_select.probe_id = None;
        self.native_select.in_flight = None;
        if self.config.frame_slot.clear_native_select_popup() {
            event_tx.wake_ui();
        }
    }

    pub(super) fn apply_native_select_choice(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        index: u32,
    ) {
        let Some(popup) = frame_slot.native_select_popup() else {
            return;
        };
        let Some(_) = popup.selectable_index(index) else {
            return;
        };
        let result = match self.send_page_command(
            link,
            event_tx,
            frame_slot,
            "Runtime.evaluate",
            &json!({
                "expression": apply_expression(&popup.css_path, index),
                "returnByValue": true,
                "awaitPromise": true,
                "userGesture": true,
            }),
        ) {
            Ok(result) => result,
            Err(error) => {
                tracing::debug!(target: "browser", "native select apply failed: {error}");
                self.dismiss_native_select(event_tx);
                return;
            }
        };
        match parse_apply(evaluation_value(Some(&result)).unwrap_or(&Value::Null)) {
            NativeSelectApply::Applied | NativeSelectApply::Unchanged => {
                self.native_select.last_popup_focused = true;
                self.dismiss_native_select(event_tx);
                self.send_escape_to_page(link);
            }
            NativeSelectApply::Blocked => {}
            NativeSelectApply::Gone => self.dismiss_native_select(event_tx),
        }
    }

    pub(super) fn queue_native_select_probe(&mut self, link: &mut CdpLink, probe: NativeSelectProbe) {
        self.native_select.pending = Some(probe);
        if let Err(error) = self.ensure_page_runtime(link) {
            tracing::debug!(target: "browser", "native select probe runtime: {error}");
            return;
        }
        self.flush_native_select_probe(link);
    }

    pub(super) fn note_native_select_input(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &FrameSlot,
        input: &BrowserInput,
    ) {
        if matches!(input, BrowserInput::Wheel { .. }) && frame_slot.native_select_popup().is_some() {
            self.dismiss_native_select(event_tx);
            self.send_escape_to_page(link);
        }
        if matches!(
            input,
            BrowserInput::KeyDown {
                key: BrowserKey::Escape,
                ..
            }
        ) && frame_slot.native_select_popup().is_some()
        {
            self.dismiss_native_select(event_tx);
            return;
        }
        if let Some(probe) = NativeSelectProbe::from_input(input, self.native_select.last_popup_focused) {
            self.queue_native_select_probe(link, probe);
        }
    }

    pub(super) fn flush_native_select_probe(&mut self, link: &mut CdpLink) {
        if self.native_select.probe_id.is_some() {
            return;
        }
        let Some(session) = self.session_id.clone() else {
            return;
        };
        if !self.runtime_enable_requested.contains(&session)
            || self.runtime_enable_inflight.values().any(|pending| pending == &session)
        {
            return;
        }
        let Some(probe) = self.native_select.pending.take() else {
            return;
        };
        match link.send_request(
            "Runtime.evaluate",
            &json!({
                "expression": probe_expression(probe),
                "returnByValue": true,
                "awaitPromise": true,
                "userGesture": true,
            }),
            Some(session.as_str()),
        ) {
            Ok(request_id) => {
                self.native_select.probe_id = Some(request_id);
                self.native_select.in_flight = Some(probe);
            }
            Err(error) => tracing::debug!(target: "browser", "native select probe failed: {error}"),
        }
    }

    pub(super) fn handle_native_select_response(
        &mut self,
        id: u64,
        result: Option<&Value>,
        error: Option<&CdpErrorInfo>,
        event_tx: &BrowserEventSender,
        link: &mut CdpLink,
    ) -> bool {
        if self.native_select.probe_id != Some(id) {
            return false;
        }
        self.native_select.probe_id = None;
        let probe = self.native_select.in_flight.take();
        if let Some(error) = error {
            tracing::debug!(target: "browser", "native select probe rejected: {error}");
        } else {
            self.publish_native_select_probe(evaluation_value(result).unwrap_or(&Value::Null), probe, event_tx, link);
        }
        self.flush_native_select_probe(link);
        true
    }

    pub(super) fn send_escape_to_page(&mut self, link: &mut CdpLink) {
        let Some(session) = self.session_id.clone() else {
            return;
        };
        let modifiers = BrowserModifiers::none();
        for pressed in [true, false] {
            let input = if pressed {
                BrowserInput::KeyDown {
                    physical_key: Some(BrowserKey::Escape),
                    key: BrowserKey::Escape,
                    text: None,
                    modifiers,
                    repeat: false,
                    edit_command: None,
                }
            } else {
                BrowserInput::KeyUp {
                    physical_key: Some(BrowserKey::Escape),
                    key: BrowserKey::Escape,
                    text: None,
                    modifiers,
                }
            };
            let (method, params) = input.cdp();
            if let Err(error) = link.send_request(method, &params, Some(session.as_str())) {
                tracing::debug!(target: "browser", "native select escape failed: {error}");
                return;
            }
        }
    }

    fn publish_native_select_probe(
        &mut self,
        value: &Value,
        probe: Option<NativeSelectProbe>,
        event_tx: &BrowserEventSender,
        link: &mut CdpLink,
    ) {
        let Some(popup) = parse_probe(value) else {
            self.native_select.last_popup_focused = false;
            if self.config.frame_slot.native_select_popup().is_some() {
                self.dismiss_native_select(event_tx);
                self.send_escape_to_page(link);
            }
            return;
        };
        self.native_select.last_popup_focused = true;
        if !probe.is_some_and(NativeSelectProbe::should_open) {
            return;
        }
        if self
            .config
            .frame_slot
            .native_select_popup()
            .is_some_and(|open| open.css_path == popup.css_path)
        {
            self.dismiss_native_select(event_tx);
            self.send_escape_to_page(link);
            return;
        }
        if self.config.frame_slot.publish_native_select_popup(popup) {
            event_tx.wake_ui();
        }
    }
}

fn evaluation_value(result: Option<&Value>) -> Option<&Value> {
    result
        .and_then(|value| value.pointer("/result/value"))
        .or_else(|| result.and_then(|value| value.get("value")))
}
