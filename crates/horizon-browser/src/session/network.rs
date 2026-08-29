//! Chromium CDP network-event ingestion for explicit MCP capture exports.

use std::sync::Arc;

use crate::cdp::{CdpEvent, CdpLink};
use crate::frames::FrameSlot;
use crate::network::{NetworkCaptureHost, decoded_base64_len};
use crate::{
    AgentAction, BrowserControlFailure, BrowserControlValue, BrowserNetworkCaptureOptions, BrowserNetworkDirection,
    BrowserNetworkEventKind, BrowserNetworkOperation, BrowserNetworkPayloadEncoding,
};

use super::{BrowserEvent, BrowserEventSender, DriverState};

const MAX_HTTP_BODY_FETCHES_PER_TICK: usize = 4;
const MAX_HTTP_BODY_FLUSH_BATCHES: usize = 16;
const MAX_PENDING_HTTP_BODIES: usize = 4_096;

impl DriverState {
    pub(super) fn network_action(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        request: &AgentAction,
        operation: BrowserNetworkOperation,
        options: Option<BrowserNetworkCaptureOptions>,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let capture = match operation {
            BrowserNetworkOperation::Start => {
                self.pending_http_bodies.clear();
                let session = self.session_id.clone().ok_or_else(|| {
                    BrowserControlFailure::new("browser_unavailable", "the Chromium page session is not attached")
                })?;
                self.network.start(
                    NetworkCaptureHost::new(
                        self.config.capture_directory.as_deref(),
                        self.config.coordination.as_deref(),
                        &self.config.panel_local_id,
                    ),
                    &request.action_id,
                    crate::BackendKind::ChromiumCdp,
                    "cdp",
                    options.unwrap_or_default(),
                )?;
                if let Err(error) = self.call_and_ack(
                    link,
                    event_tx,
                    frame_slot,
                    "Network.enable",
                    &serde_json::json!({}),
                    Some(session.as_str()),
                ) {
                    let _ = self.network.stop();
                    return Err(BrowserControlFailure::new(
                        "capture_protocol",
                        format!("Chromium could not enable network observation: {error}"),
                    ));
                }
                self.network.status()?
            }
            BrowserNetworkOperation::Status => self.network.status()?,
            BrowserNetworkOperation::Stop => {
                self.flush_http_response_bodies(link, event_tx, frame_slot);
                if self.network.is_active()
                    && let Some(session) = self.session_id.clone()
                    && let Err(error) = self.call_and_ack(
                        link,
                        event_tx,
                        frame_slot,
                        "Network.disable",
                        &serde_json::json!({}),
                        Some(session.as_str()),
                    )
                {
                    tracing::warn!(target: "browser", "Chromium network disable failed before capture flush: {error}");
                }
                self.network.stop()?
            }
        };
        Ok(BrowserControlValue::Network { capture })
    }

    /// A target reattach clears domain enablement. Restore an active capture
    /// without making page recovery depend on optional observation state.
    pub(super) fn restore_network_capture(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
        session: &str,
    ) {
        if !self.network.is_active() {
            return;
        }
        self.abandon_http_response_bodies("Chromium target reattached before response-body retrieval");
        if let Err(error) = self.call_and_ack(
            link,
            event_tx,
            frame_slot,
            "Network.enable",
            &serde_json::json!({}),
            Some(session),
        ) {
            let _ = self.network.stop();
            let _ = event_tx.send(BrowserEvent::Warning(format!(
                "browser network capture stopped after target reattach: {error}"
            )));
        }
    }

    pub(super) fn handle_network_event(&mut self, event: &CdpEvent<'_>) {
        if !self.network.is_active() {
            return;
        }
        if self.handle_http_network_event(event) {
            return;
        }
        match event.method {
            "Network.webSocketCreated" => {
                if let (Some(connection_id), Some(url)) =
                    (string_at(event.params, "/requestId"), string_at(event.params, "/url"))
                {
                    self.network.record_websocket_created(connection_id, url);
                }
            }
            "Network.webSocketHandshakeResponseReceived" => {
                if let Some(connection_id) = string_at(event.params, "/requestId") {
                    self.network.record_websocket_opened(connection_id, None, false);
                }
            }
            "Network.webSocketFrameSent" => {
                self.record_cdp_websocket_frame(event.params, BrowserNetworkDirection::Sent);
            }
            "Network.webSocketFrameReceived" => {
                self.record_cdp_websocket_frame(event.params, BrowserNetworkDirection::Received);
            }
            "Network.webSocketFrameError" => {
                if let Some(connection_id) = string_at(event.params, "/requestId") {
                    self.network.record_websocket_terminal(
                        connection_id,
                        BrowserNetworkEventKind::WebsocketError,
                        string_at(event.params, "/errorMessage"),
                    );
                }
            }
            "Network.webSocketClosed" => {
                if let Some(connection_id) = string_at(event.params, "/requestId") {
                    self.network.record_websocket_terminal(
                        connection_id,
                        BrowserNetworkEventKind::WebsocketClosed,
                        None,
                    );
                }
            }
            _ => {}
        }
    }

    fn handle_http_network_event(&mut self, event: &CdpEvent<'_>) -> bool {
        match event.method {
            "Network.requestWillBeSent" => {
                self.network.record_http(
                    BrowserNetworkEventKind::HttpRequest,
                    string_at(event.params, "/requestId"),
                    string_at(event.params, "/request/url"),
                    string_at(event.params, "/request/method"),
                    None,
                    string_at(event.params, "/type"),
                    None,
                    None,
                );
                true
            }
            "Network.responseReceived" => {
                self.network.record_http(
                    BrowserNetworkEventKind::HttpResponse,
                    string_at(event.params, "/requestId"),
                    string_at(event.params, "/response/url"),
                    None,
                    u16_at(event.params, "/response/status"),
                    string_at(event.params, "/type"),
                    None,
                    None,
                );
                true
            }
            "Network.loadingFinished" => {
                let request_id = string_at(event.params, "/requestId");
                let body = request_id.and_then(|request_id| {
                    self.network
                        .http_body_url(request_id)
                        .map(|url| (request_id.to_string(), url))
                });
                self.network.record_http(
                    BrowserNetworkEventKind::HttpCompleted,
                    request_id,
                    None,
                    None,
                    None,
                    None,
                    u64_at(event.params, "/encodedDataLength"),
                    None,
                );
                if let Some(body) = body {
                    if self.pending_http_bodies.len() < MAX_PENDING_HTTP_BODIES {
                        self.pending_http_bodies.push_back(body);
                    } else {
                        self.network.record_http_body(
                            &body.0,
                            &body.1,
                            None,
                            None,
                            None,
                            false,
                            Some("Chromium response-body queue limit reached"),
                        );
                    }
                }
                true
            }
            "Network.loadingFailed" => {
                self.network.record_http(
                    BrowserNetworkEventKind::HttpFailed,
                    string_at(event.params, "/requestId"),
                    None,
                    None,
                    None,
                    string_at(event.params, "/type"),
                    None,
                    string_at(event.params, "/errorText"),
                );
                true
            }
            _ => false,
        }
    }

    pub(super) fn tick_http_response_bodies(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        for _ in 0..MAX_HTTP_BODY_FETCHES_PER_TICK {
            let Some((request_id, url)) = self.pending_http_bodies.pop_front() else {
                break;
            };
            let Some(session) = self.session_id.clone() else {
                self.network.record_http_body(
                    &request_id,
                    &url,
                    None,
                    None,
                    None,
                    false,
                    Some("Chromium page session detached before response-body retrieval"),
                );
                continue;
            };
            match self.call_and_ack(
                link,
                event_tx,
                frame_slot,
                "Network.getResponseBody",
                &serde_json::json!({ "requestId": request_id }),
                Some(session.as_str()),
            ) {
                Ok(result) => self.record_cdp_http_body(&request_id, &url, &result),
                Err(error) => {
                    self.network
                        .record_http_body(&request_id, &url, None, None, None, false, Some(&error.to_string()));
                }
            }
        }
    }

    pub(super) fn flush_http_response_bodies(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        for _ in 0..MAX_HTTP_BODY_FLUSH_BATCHES {
            if self.pending_http_bodies.is_empty() {
                return;
            }
            self.tick_http_response_bodies(link, event_tx, frame_slot);
        }
        self.abandon_http_response_bodies("Chromium capture stopped before response-body retrieval");
    }

    fn abandon_http_response_bodies(&mut self, reason: &str) {
        while let Some((request_id, url)) = self.pending_http_bodies.pop_front() {
            self.network
                .record_http_body(&request_id, &url, None, None, None, false, Some(reason));
        }
    }

    fn record_cdp_http_body(&mut self, request_id: &str, url: &str, result: &serde_json::Value) {
        let Some(payload) = result.get("body").and_then(serde_json::Value::as_str) else {
            self.network.record_http_body(
                request_id,
                url,
                None,
                None,
                None,
                false,
                Some("Chromium omitted the response body"),
            );
            return;
        };
        let encoding = if result
            .get("base64Encoded")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            BrowserNetworkPayloadEncoding::Base64
        } else {
            BrowserNetworkPayloadEncoding::Text
        };
        let payload_bytes = match encoding {
            BrowserNetworkPayloadEncoding::Text => u64::try_from(payload.len()).unwrap_or(u64::MAX),
            BrowserNetworkPayloadEncoding::Base64 => decoded_base64_len(payload),
        };
        self.network.record_http_body(
            request_id,
            url,
            Some(payload),
            Some(encoding),
            Some(payload_bytes),
            false,
            None,
        );
    }

    fn record_cdp_websocket_frame(&mut self, params: &serde_json::Value, direction: BrowserNetworkDirection) {
        let Some(connection_id) = string_at(params, "/requestId") else {
            return;
        };
        let opcode = u8_at(params, "/response/opcode");
        let payload = string_at(params, "/response/payloadData");
        let encoding = if opcode == Some(2) {
            BrowserNetworkPayloadEncoding::Base64
        } else {
            BrowserNetworkPayloadEncoding::Text
        };
        let payload_bytes = payload.map_or(0, |value| match encoding {
            BrowserNetworkPayloadEncoding::Text => u64::try_from(value.len()).unwrap_or(u64::MAX),
            BrowserNetworkPayloadEncoding::Base64 => decoded_base64_len(value),
        });
        self.network.record_websocket_frame(
            connection_id,
            None,
            direction,
            opcode,
            payload,
            encoding,
            payload_bytes,
            false,
        );
    }
}

fn string_at<'a>(value: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(serde_json::Value::as_str)
}

fn u64_at(value: &serde_json::Value, pointer: &str) -> Option<u64> {
    value.pointer(pointer).and_then(serde_json::Value::as_u64)
}

fn u16_at(value: &serde_json::Value, pointer: &str) -> Option<u16> {
    u64_at(value, pointer).and_then(|value| u16::try_from(value).ok())
}

fn u8_at(value: &serde_json::Value, pointer: &str) -> Option<u8> {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u8::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_numeric_helpers_reject_invalid_values() {
        let value = serde_json::json!({ "ok": 101, "negative": -1, "fractional": 1.5, "opcode": 2 });
        assert_eq!(u16_at(&value, "/ok"), Some(101));
        assert_eq!(u64_at(&value, "/negative"), None);
        assert_eq!(u64_at(&value, "/fractional"), None);
        assert_eq!(u8_at(&value, "/opcode"), Some(2));
    }
}
