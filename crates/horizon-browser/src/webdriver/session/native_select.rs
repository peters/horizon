//! `WebDriver` host overlay for native size-1 `<select>` popups.

use serde_json::{Value, json};

use crate::native_select::{
    NativeSelectApply, NativeSelectProbe, apply_expression, parse_apply, parse_probe, probe_expression,
};
use crate::session::BrowserEventSender;
use crate::{BrowserInput, BrowserKey, BrowserModifiers};

use super::Driver;
use super::webdriver_value;

#[derive(Debug, Default)]
pub(crate) struct NativeSelectState {
    last_popup_focused: bool,
}

impl Driver {
    pub(super) fn dismiss_native_select(&mut self, event_tx: &BrowserEventSender) {
        if self.panel_slot.clear_native_select_popup() {
            event_tx.wake_ui();
        }
    }

    pub(super) fn apply_native_select_choice(
        &mut self,
        event_tx: &BrowserEventSender,
        index: u32,
    ) -> Result<(), String> {
        let Some(popup) = self.panel_slot.native_select_popup() else {
            return Ok(());
        };
        if popup.selectable_index(index).is_none() {
            return Ok(());
        }
        let response = self.classic_post(
            "execute/sync",
            &json!({
                "script": format!("return ({});", apply_expression(&popup.css_path, index)),
                "args": []
            }),
        )?;
        let value = webdriver_value(&response).cloned().unwrap_or(Value::Null);
        match parse_apply(&value) {
            NativeSelectApply::Applied | NativeSelectApply::Unchanged => {
                self.native_select.last_popup_focused = true;
                self.dismiss_native_select(event_tx);
                self.send_escape_to_page(event_tx);
            }
            NativeSelectApply::Blocked => {}
            NativeSelectApply::Gone => self.dismiss_native_select(event_tx),
        }
        self.frames.demand();
        Ok(())
    }

    pub(super) fn note_native_select_input(
        &mut self,
        input: &BrowserInput,
        event_tx: &BrowserEventSender,
    ) -> Result<(), String> {
        if matches!(input, BrowserInput::Wheel { .. }) && self.panel_slot.native_select_popup().is_some() {
            self.dismiss_native_select(event_tx);
            self.send_escape_to_page(event_tx);
        }
        if matches!(
            input,
            BrowserInput::KeyDown {
                key: BrowserKey::Escape,
                ..
            }
        ) && self.panel_slot.native_select_popup().is_some()
        {
            self.dismiss_native_select(event_tx);
            return Ok(());
        }
        let Some(probe) = NativeSelectProbe::from_input(input, self.native_select.last_popup_focused) else {
            return Ok(());
        };
        self.probe_native_select(probe, event_tx)
    }

    pub(super) fn send_escape_to_page(&mut self, event_tx: &BrowserEventSender) {
        let modifiers = BrowserModifiers::none();
        let down = BrowserInput::KeyDown {
            physical_key: Some(BrowserKey::Escape),
            key: BrowserKey::Escape,
            text: None,
            modifiers,
            repeat: false,
            edit_command: None,
        };
        let up = BrowserInput::KeyUp {
            physical_key: Some(BrowserKey::Escape),
            key: BrowserKey::Escape,
            text: None,
            modifiers,
        };
        for input in [down, up] {
            if self.firefox_bidi() {
                let mut payload = self.actions.payload(input);
                payload["context"] = json!(self.context_id);
                if let Err(error) = self.call_bidi("input.performActions", &payload, event_tx) {
                    tracing::debug!(target: "browser", "native select escape failed: {error}");
                    return;
                }
            } else {
                let payload = self.actions.payload(input);
                if let Err(error) = self.classic_post("actions", &payload) {
                    tracing::debug!(target: "browser", "native select escape failed: {error}");
                    return;
                }
            }
        }
    }

    fn probe_native_select(&mut self, probe: NativeSelectProbe, event_tx: &BrowserEventSender) -> Result<(), String> {
        let response = self.classic_post(
            "execute/sync",
            &json!({
                "script": format!("return ({});", probe_expression(probe)),
                "args": []
            }),
        )?;
        let value = webdriver_value(&response).cloned().unwrap_or(Value::Null);
        let Some(popup) = parse_probe(&value) else {
            self.native_select.last_popup_focused = false;
            if self.panel_slot.native_select_popup().is_some() {
                self.dismiss_native_select(event_tx);
                self.send_escape_to_page(event_tx);
            }
            return Ok(());
        };
        self.native_select.last_popup_focused = true;
        if !probe.should_open() {
            return Ok(());
        }
        if self
            .panel_slot
            .native_select_popup()
            .is_some_and(|open| open.css_path == popup.css_path)
        {
            self.dismiss_native_select(event_tx);
            self.send_escape_to_page(event_tx);
            return Ok(());
        }
        if self.panel_slot.publish_native_select_popup(popup) {
            event_tx.wake_ui();
        }
        Ok(())
    }
}
