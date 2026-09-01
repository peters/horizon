//! Chromium CDP network-event ingestion for explicit MCP capture exports.

use std::sync::Arc;

use crate::cdp::{CdpEvent, CdpLink};
use crate::frames::FrameSlot;
use crate::network::{NetworkCaptureHost, decoded_base64_len};
use crate::{
    AgentAction, BrowserControlFailure, BrowserControlValue, BrowserNetworkCaptureOptions, BrowserNetworkDirection,
    BrowserNetworkEventKind, BrowserNetworkOperation, BrowserNetworkPayloadEncoding,
};

use super::http_bodies::{CdpBodyOutcome, PendingHttpBody, decode_cdp_body};
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
                self.http_body_evidence.clear();
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
        self.http_body_evidence.clear();
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
            "Network.requestWillBeSent" => self.on_http_request(event.params),
            "Network.responseReceived" => self.on_http_response(event.params),
            "Network.dataReceived" => {
                if let (Some(request_id), Some(data_length)) = (
                    string_at(event.params, "/requestId"),
                    u64_at(event.params, "/dataLength"),
                ) {
                    self.http_body_evidence.note_data(request_id, data_length);
                }
            }
            "Network.loadingFinished" => self.on_http_completed(event.params),
            "Network.loadingFailed" => self.on_http_failed(event.params),
            _ => return false,
        }
        true
    }

    fn on_http_request(&mut self, params: &serde_json::Value) {
        let request_id = string_at(params, "/requestId");
        let method = string_at(params, "/request/method");
        self.network.record_http(
            BrowserNetworkEventKind::HttpRequest,
            request_id,
            string_at(params, "/request/url"),
            method,
            None,
            string_at(params, "/type"),
            None,
            None,
        );
        if let Some(request_id) = request_id
            && self.network.http_body_url(request_id).is_some()
        {
            self.http_body_evidence.begin(request_id, method);
        }
    }

    fn on_http_response(&mut self, params: &serde_json::Value) {
        let request_id = string_at(params, "/requestId");
        self.network.record_http(
            BrowserNetworkEventKind::HttpResponse,
            request_id,
            string_at(params, "/response/url"),
            None,
            u16_at(params, "/response/status"),
            string_at(params, "/type"),
            None,
            None,
        );
        if let (Some(request_id), Some(response)) = (request_id, params.get("response")) {
            if self.network.http_body_url(request_id).is_some() {
                self.http_body_evidence.note_response(request_id, response);
            } else {
                self.http_body_evidence.forget(request_id);
            }
        }
    }

    fn on_http_completed(&mut self, params: &serde_json::Value) {
        let request_id = string_at(params, "/requestId");
        let encoded_data_length = u64_at(params, "/encodedDataLength");
        let body = request_id.and_then(|request_id| {
            let evidence = self.http_body_evidence.finish(request_id, encoded_data_length);
            self.network.http_body_url(request_id).map(|url| PendingHttpBody {
                request_id: request_id.to_string(),
                url,
                evidence,
            })
        });
        self.network.record_http(
            BrowserNetworkEventKind::HttpCompleted,
            request_id,
            None,
            None,
            None,
            None,
            encoded_data_length,
            None,
        );
        let Some(body) = body else {
            return;
        };
        if self.pending_http_bodies.len() < MAX_PENDING_HTTP_BODIES {
            self.pending_http_bodies.push_back(body);
        } else {
            self.network.record_http_body(
                &body.request_id,
                &body.url,
                None,
                None,
                None,
                false,
                Some("Chromium response-body queue limit reached"),
            );
        }
    }

    fn on_http_failed(&mut self, params: &serde_json::Value) {
        let request_id = string_at(params, "/requestId");
        if let Some(request_id) = request_id {
            self.http_body_evidence.forget(request_id);
        }
        self.network.record_http(
            BrowserNetworkEventKind::HttpFailed,
            request_id,
            None,
            None,
            None,
            string_at(params, "/type"),
            None,
            string_at(params, "/errorText"),
        );
    }

    pub(super) fn tick_http_response_bodies(
        &mut self,
        link: &mut CdpLink,
        event_tx: &BrowserEventSender,
        frame_slot: &Arc<FrameSlot>,
    ) {
        for _ in 0..MAX_HTTP_BODY_FETCHES_PER_TICK {
            let Some(pending) = self.pending_http_bodies.pop_front() else {
                break;
            };
            let Some(session) = self.session_id.clone() else {
                self.network.record_http_body(
                    &pending.request_id,
                    &pending.url,
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
                &serde_json::json!({ "requestId": pending.request_id }),
                Some(session.as_str()),
            ) {
                Ok(result) => self.record_cdp_http_body(&pending, &result),
                Err(error) => {
                    self.network.record_http_body(
                        &pending.request_id,
                        &pending.url,
                        None,
                        None,
                        None,
                        false,
                        Some(&error.to_string()),
                    );
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
        while let Some(pending) = self.pending_http_bodies.pop_front() {
            self.network
                .record_http_body(&pending.request_id, &pending.url, None, None, None, false, Some(reason));
        }
    }

    fn record_cdp_http_body(&mut self, pending: &PendingHttpBody, result: &serde_json::Value) {
        match decode_cdp_body(pending.evidence.as_ref(), result) {
            CdpBodyOutcome::Captured {
                payload,
                encoding,
                payload_bytes,
            } => self.network.record_http_body(
                &pending.request_id,
                &pending.url,
                Some(payload),
                Some(encoding),
                Some(payload_bytes),
                false,
                None,
            ),
            CdpBodyOutcome::Unavailable(reason) => self.network.record_http_body(
                &pending.request_id,
                &pending.url,
                None,
                None,
                None,
                false,
                Some(&reason),
            ),
        }
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
