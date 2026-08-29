//! Firefox `WebDriver` `BiDi` network capture bridge.
//!
//! `BiDi` exposes HTTP lifecycle events but does not expose WebSocket frames.
//! Firefox therefore uses an explicitly reported pre-document page bridge for
//! WebSocket payloads. The bridge batches records before crossing `BiDi` and the
//! Rust side writes through the same bounded, non-blocking capture pipeline as
//! Chromium CDP.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AgentAction, BackendKind, BrowserControlFailure, BrowserControlValue, BrowserNetworkCaptureOptions,
    BrowserNetworkDirection, BrowserNetworkEventKind, BrowserNetworkOperation, BrowserNetworkPayloadEncoding,
};

use super::{BrowserEventSender, Driver};

const MAX_HTTP_BODY_FETCHES_PER_TICK: usize = 16;
const MAX_HTTP_BODY_FLUSH_BATCHES: usize = 4;
const MAX_PENDING_HTTP_BODIES: usize = 4_096;
const PAGE_BRIDGE_TEMPLATE: &str = r#"(emit) => {
    const controlKey = __CONTROL_KEY__;
    if (globalThis[controlKey]) return;
    const NativeWebSocket = globalThis.WebSocket;
    if (typeof NativeWebSocket !== "function") return;
    const patterns = __URL_PATTERNS__;
    const includeSent = __INCLUDE_SENT__;
    const includeReceived = __INCLUDE_RECEIVED__;
    const maxPayloadBytes = __MAX_PAYLOAD_BYTES__;
    const encoder = new TextEncoder();
    const decoder = new TextDecoder();
    const originalDescriptor = Object.getOwnPropertyDescriptor(globalThis, "WebSocket");
    let nextId = 0;
    let queue = [];
    let queuedChars = 0;
    let timer;
    let stopped = false;
    const cleanups = new Set();

    const flush = () => {
        if (timer) clearTimeout(timer);
        timer = undefined;
        if (!queue.length) return;
        const batch = queue.splice(0, queue.length);
        queuedChars = 0;
        try { emit(JSON.stringify(batch)); } catch (_) {}
    };
    const push = (record) => {
        if (stopped) return;
        queue.push(record);
        queuedChars += 256 + (record.payload ? record.payload.length : 0);
        if (queue.length >= 64 || queuedChars >= 262144) flush();
        else if (!timer) timer = setTimeout(flush, 20);
    };
    const matches = (url) => !patterns.length || patterns.some((pattern) => url.includes(pattern));
    const base64 = (bytes) => {
        let binary = "";
        for (let offset = 0; offset < bytes.length; offset += 8192) {
            binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
        }
        return btoa(binary);
    };
    const byteRecord = (bytes) => {
        const bounded = bytes.subarray(0, maxPayloadBytes);
        return {
            payload: base64(bounded),
            encoding: "base64",
            payload_bytes: bytes.byteLength,
            truncated: bytes.byteLength > bounded.byteLength,
            opcode: 2
        };
    };
    const textRecord = (text) => {
        const bytes = encoder.encode(text);
        const bounded = bytes.subarray(0, maxPayloadBytes);
        return {
            payload: decoder.decode(bounded),
            encoding: "text",
            payload_bytes: bytes.byteLength,
            truncated: bytes.byteLength > bounded.byteLength,
            opcode: 1
        };
    };
    const emitFrame = (meta, direction, data) => {
        const finish = (payload) => push({ kind: "frame", connection_id: meta.id, direction, ...payload });
        if (typeof data === "string") {
            finish(textRecord(data));
        } else if (data instanceof Blob) {
            data.arrayBuffer().then((buffer) => finish(byteRecord(new Uint8Array(buffer)))).catch(() => {});
        } else if (data instanceof ArrayBuffer) {
            finish(byteRecord(new Uint8Array(data)));
        } else if (ArrayBuffer.isView(data)) {
            finish(byteRecord(new Uint8Array(data.buffer, data.byteOffset, data.byteLength)));
        }
    };
    const observe = (socket) => {
        const url = String(socket.url || "");
        if (!matches(url)) return;
        const meta = { id: `firefox-${Date.now()}-${++nextId}`, url };
        push({ kind: "created", connection_id: meta.id, url });
        const onOpen = () => push({ kind: "opened", connection_id: meta.id, url });
        const onError = () => push({ kind: "error", connection_id: meta.id, error: "WebSocket error" });
        const onMessage = (event) => emitFrame(meta, "received", event.data);
        const ownSendDescriptor = Object.getOwnPropertyDescriptor(socket, "send");
        let sendWrapped = false;
        let cleanup;
        const onClose = (event) => {
            push({
            kind: "closed",
            connection_id: meta.id,
            error: event.reason || undefined
            });
            cleanup();
        };
        cleanup = () => {
            socket.removeEventListener("open", onOpen);
            socket.removeEventListener("error", onError);
            socket.removeEventListener("close", onClose);
            if (includeReceived) socket.removeEventListener("message", onMessage);
            if (sendWrapped) {
                if (ownSendDescriptor) Reflect.defineProperty(socket, "send", ownSendDescriptor);
                else delete socket.send;
            }
            cleanups.delete(cleanup);
        };
        socket.addEventListener("open", onOpen, { once: true });
        socket.addEventListener("error", onError);
        socket.addEventListener("close", onClose, { once: true });
        if (includeReceived) {
            socket.addEventListener("message", onMessage);
        }
        if (includeSent) {
            const nativeSend = socket.send;
            try {
                Object.defineProperty(socket, "send", {
                    configurable: true,
                    writable: true,
                    value(data) {
                        const result = Reflect.apply(nativeSend, this, [data]);
                        emitFrame(meta, "sent", data);
                        return result;
                    }
                });
                sendWrapped = true;
            } catch (_) {}
        }
        cleanups.add(cleanup);
    };
    let WrappedWebSocket;
    WrappedWebSocket = new Proxy(NativeWebSocket, {
        construct(target, args, newTarget) {
            const socket = Reflect.construct(target, args, newTarget === WrappedWebSocket ? target : newTarget);
            observe(socket);
            return socket;
        }
    });
    const replacement = {
        configurable: originalDescriptor ? originalDescriptor.configurable : true,
        enumerable: originalDescriptor ? originalDescriptor.enumerable : false,
        writable: originalDescriptor && "writable" in originalDescriptor ? originalDescriptor.writable : true,
        value: WrappedWebSocket
    };
    if (!Reflect.defineProperty(globalThis, "WebSocket", replacement)) return;
    const control = {
        stop() {
            stopped = true;
            for (const cleanup of [...cleanups]) cleanup();
            flush();
            if (originalDescriptor) Reflect.defineProperty(globalThis, "WebSocket", originalDescriptor);
            else delete globalThis.WebSocket;
            delete globalThis[controlKey];
        }
    };
    if (!Reflect.defineProperty(globalThis, controlKey, {
        configurable: true,
        enumerable: false,
        writable: false,
        value: control
    })) {
        stopped = true;
        for (const cleanup of [...cleanups]) cleanup();
        if (originalDescriptor) Reflect.defineProperty(globalThis, "WebSocket", originalDescriptor);
        else delete globalThis.WebSocket;
        throw new Error("WebSocket capture control could not be installed");
    }
}"#;

#[derive(Debug)]
pub(super) struct FirefoxNetworkBridge {
    page: Option<FirefoxPageNetworkBridge>,
    subscription: String,
    data_collector: Option<String>,
}

#[derive(Debug)]
struct FirefoxPageNetworkBridge {
    channel: String,
    control_key: String,
    preload_script: String,
}

#[derive(Debug, Deserialize)]
struct PageNetworkEvent {
    kind: String,
    connection_id: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    direction: Option<BrowserNetworkDirection>,
    #[serde(default)]
    opcode: Option<u8>,
    #[serde(default)]
    payload: Option<String>,
    #[serde(default)]
    encoding: Option<BrowserNetworkPayloadEncoding>,
    #[serde(default)]
    payload_bytes: u64,
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    error: Option<String>,
}

impl Driver {
    pub(super) fn network_action(
        &mut self,
        request: &AgentAction,
        operation: BrowserNetworkOperation,
        options: Option<BrowserNetworkCaptureOptions>,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        if self.config.browser.backend == BackendKind::SafariWebDriver {
            return Err(BrowserControlFailure::new(
                "unsupported_backend",
                "Safari does not expose network capture through Horizon's current WebDriver transport",
            ));
        }
        let capture = match operation {
            BrowserNetworkOperation::Start => {
                self.pending_http_bodies.clear();
                let options = options.unwrap_or_default();
                self.network.start(
                    crate::network::NetworkCaptureHost::new(
                        self.config.capture_directory.as_deref(),
                        self.config.coordination.as_deref(),
                        &self.config.panel_local_id,
                    ),
                    &request.action_id,
                    BackendKind::FirefoxBidi,
                    firefox_network_transport(&options),
                    options.clone(),
                )?;
                if let Err(error) = self.install_firefox_network_bridge(&request.action_id, &options, event_tx) {
                    let _ = self.network.stop();
                    return Err(error);
                }
                self.network.status()?
            }
            BrowserNetworkOperation::Status => self.network.status()?,
            BrowserNetworkOperation::Stop => {
                if self.network.is_active() {
                    self.flush_firefox_http_response_bodies(event_tx);
                    self.remove_firefox_network_bridge(event_tx);
                }
                self.network.stop()?
            }
        };
        Ok(BrowserControlValue::Network { capture })
    }

    pub(super) fn handle_network_bidi_event(&mut self, event: &Value) -> bool {
        let method = event.get("method").and_then(Value::as_str).unwrap_or_default();
        let params = event.get("params").unwrap_or(&Value::Null);
        if method == "script.message" {
            let Some(page) = self.firefox_network.as_ref().and_then(|bridge| bridge.page.as_ref()) else {
                return false;
            };
            if params.get("channel").and_then(Value::as_str) != Some(page.channel.as_str()) {
                return false;
            }
            let Some(payload) = params.pointer("/data/value").and_then(Value::as_str) else {
                return true;
            };
            match serde_json::from_str::<Vec<PageNetworkEvent>>(payload) {
                Ok(records) => {
                    for record in records {
                        self.record_page_network_event(&record);
                    }
                }
                Err(error) => tracing::warn!(target: "browser", "invalid Firefox network bridge batch: {error}"),
            }
            return true;
        }
        match method {
            "network.beforeRequestSent" => self.network.record_http(
                BrowserNetworkEventKind::HttpRequest,
                string_at(params, "/request/request"),
                string_at(params, "/request/url"),
                string_at(params, "/request/method"),
                None,
                string_at(params, "/request/destination"),
                None,
                None,
            ),
            "network.responseStarted" => self.network.record_http(
                BrowserNetworkEventKind::HttpResponse,
                string_at(params, "/request/request"),
                string_at(params, "/response/url"),
                None,
                u16_at(params, "/response/status"),
                None,
                None,
                None,
            ),
            "network.responseCompleted" => {
                let request_id = string_at(params, "/request/request");
                let body_url = request_id.and_then(|request_id| self.network.http_body_url(request_id));
                let collects_bodies = self
                    .firefox_network
                    .as_ref()
                    .and_then(|bridge| bridge.data_collector.as_ref())
                    .is_some();
                self.network.record_http(
                    BrowserNetworkEventKind::HttpCompleted,
                    request_id,
                    string_at(params, "/response/url"),
                    None,
                    u16_at(params, "/response/status"),
                    None,
                    u64_at(params, "/response/bodySize"),
                    None,
                );
                if collects_bodies && let Some(request_id) = request_id {
                    if self.pending_http_bodies.len() < MAX_PENDING_HTTP_BODIES {
                        self.pending_http_bodies.push_back((request_id.to_string(), body_url));
                    } else if let Some(url) = body_url {
                        self.network.record_http_body(
                            request_id,
                            &url,
                            None,
                            None,
                            None,
                            false,
                            Some("Firefox response-body queue limit reached"),
                        );
                    }
                }
            }
            "network.fetchError" => self.network.record_http(
                BrowserNetworkEventKind::HttpFailed,
                string_at(params, "/request/request"),
                string_at(params, "/request/url"),
                None,
                None,
                None,
                None,
                string_at(params, "/errorText"),
            ),
            _ => {}
        }
        false
    }

    pub(super) fn tick_firefox_http_response_bodies(&mut self, event_tx: &BrowserEventSender) {
        let Some(collector) = self
            .firefox_network
            .as_ref()
            .and_then(|bridge| bridge.data_collector.clone())
        else {
            self.pending_http_bodies.clear();
            return;
        };
        for _ in 0..MAX_HTTP_BODY_FETCHES_PER_TICK {
            let Some((request_id, url)) = self.pending_http_bodies.pop_front() else {
                break;
            };
            let params = json!({
                "dataType": "response",
                "collector": collector,
                "request": request_id,
            });
            let Some(url) = url else {
                let _ = self.call_bidi("network.disownData", &params, event_tx);
                continue;
            };
            let mut get_params = params;
            if let Some(object) = get_params.as_object_mut() {
                object.insert("disown".to_string(), Value::Bool(true));
            }
            match self.call_bidi("network.getData", &get_params, event_tx) {
                Ok(result) => self.record_firefox_http_body(&request_id, &url, &result),
                Err(error) => self
                    .network
                    .record_http_body(&request_id, &url, None, None, None, false, Some(&error)),
            }
        }
    }

    pub(super) fn flush_firefox_http_response_bodies(&mut self, event_tx: &BrowserEventSender) {
        for _ in 0..MAX_HTTP_BODY_FLUSH_BATCHES {
            if self.pending_http_bodies.is_empty() {
                return;
            }
            self.tick_firefox_http_response_bodies(event_tx);
        }
        self.abandon_firefox_http_response_bodies("Firefox capture stopped before response-body retrieval");
    }

    fn abandon_firefox_http_response_bodies(&mut self, reason: &str) {
        while let Some((request_id, url)) = self.pending_http_bodies.pop_front() {
            if let Some(url) = url {
                self.network
                    .record_http_body(&request_id, &url, None, None, None, false, Some(reason));
            }
        }
    }

    fn record_firefox_http_body(&mut self, request_id: &str, url: &str, result: &Value) {
        let Some(kind) = string_at(result, "/bytes/type") else {
            self.network.record_http_body(
                request_id,
                url,
                None,
                None,
                None,
                false,
                Some("Firefox omitted the response-body encoding"),
            );
            return;
        };
        let Some(payload) = string_at(result, "/bytes/value") else {
            self.network.record_http_body(
                request_id,
                url,
                None,
                None,
                None,
                false,
                Some("Firefox omitted the response body"),
            );
            return;
        };
        let encoding = match kind {
            "string" => BrowserNetworkPayloadEncoding::Text,
            "base64" => BrowserNetworkPayloadEncoding::Base64,
            _ => {
                self.network.record_http_body(
                    request_id,
                    url,
                    None,
                    None,
                    None,
                    false,
                    Some("Firefox returned an unknown response-body encoding"),
                );
                return;
            }
        };
        let payload_bytes = match encoding {
            BrowserNetworkPayloadEncoding::Text => u64::try_from(payload.len()).unwrap_or(u64::MAX),
            BrowserNetworkPayloadEncoding::Base64 => crate::network::decoded_base64_len(payload),
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

    fn install_firefox_network_bridge(
        &mut self,
        capture_id: &str,
        options: &BrowserNetworkCaptureOptions,
        event_tx: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        let context = self.context_id.clone().ok_or_else(|| {
            BrowserControlFailure::new(
                "browser_unavailable",
                "Firefox has no active top-level browsing context",
            )
        })?;
        if self.bidi.is_none() {
            return Err(BrowserControlFailure::new(
                "browser_unavailable",
                "Firefox WebDriver BiDi is unavailable",
            ));
        }
        let data_collector = self.add_firefox_data_collector(&context, options, event_tx)?;
        let subscription = match self.subscribe_firefox_network(&context, options, event_tx) {
            Ok(subscription) => subscription,
            Err(error) => {
                if let Some(collector) = data_collector.as_deref() {
                    self.remove_firefox_data_collector(collector, event_tx);
                }
                return Err(error);
            }
        };
        self.firefox_network = Some(FirefoxNetworkBridge {
            page: None,
            subscription,
            data_collector,
        });
        if options.include_websocket {
            self.install_firefox_page_bridge(capture_id, &context, options, event_tx)?;
        }
        Ok(())
    }

    fn add_firefox_data_collector(
        &mut self,
        context: &str,
        options: &BrowserNetworkCaptureOptions,
        event_tx: &BrowserEventSender,
    ) -> Result<Option<String>, BrowserControlFailure> {
        if !options.include_http_bodies {
            return Ok(None);
        }
        let result = self
            .call_bidi(
                "network.addDataCollector",
                &json!({
                    "dataTypes": ["response"],
                    "maxEncodedDataSize": options.max_payload_bytes,
                    "collectorType": "blob",
                    "contexts": [context],
                }),
                event_tx,
            )
            .map_err(|error| BrowserControlFailure::new("capture_protocol", error))?;
        required_string(&result, "/collector", "network data collector").map(Some)
    }

    fn subscribe_firefox_network(
        &mut self,
        context: &str,
        options: &BrowserNetworkCaptureOptions,
        event_tx: &BrowserEventSender,
    ) -> Result<String, BrowserControlFailure> {
        let events = firefox_network_events(options);
        let subscription_result = self
            .call_bidi(
                "session.subscribe",
                &json!({ "events": events, "contexts": [&context] }),
                event_tx,
            )
            .map_err(|error| BrowserControlFailure::new("capture_protocol", error))?;
        required_string(&subscription_result, "/subscription", "subscription")
    }

    fn install_firefox_page_bridge(
        &mut self,
        capture_id: &str,
        context: &str,
        options: &BrowserNetworkCaptureOptions,
        event_tx: &BrowserEventSender,
    ) -> Result<(), BrowserControlFailure> {
        let channel = format!("horizon-network-{capture_id}");
        let control_key = format!("__horizonNetworkCapture_{capture_id}");
        let function = page_bridge_function(&control_key, options);
        let channel_argument = json!({
            "type": "channel",
            "value": { "channel": channel, "ownership": "none" }
        });
        let preload_result = match self.call_bidi(
            "script.addPreloadScript",
            &json!({
                "functionDeclaration": function,
                "arguments": [&channel_argument],
                "contexts": [&context]
            }),
            event_tx,
        ) {
            Ok(result) => result,
            Err(error) => {
                self.remove_firefox_network_bridge(event_tx);
                return Err(BrowserControlFailure::new("capture_protocol", error));
            }
        };
        let preload_script = match required_string(&preload_result, "/script", "preload script") {
            Ok(value) => value,
            Err(error) => {
                self.remove_firefox_network_bridge(event_tx);
                return Err(error);
            }
        };
        let Some(bridge) = self.firefox_network.as_mut() else {
            return Err(BrowserControlFailure::new(
                "capture_protocol",
                "Firefox network registration disappeared before page instrumentation",
            ));
        };
        bridge.page = Some(FirefoxPageNetworkBridge {
            channel,
            control_key,
            preload_script,
        });
        let current_result = self.call_bidi(
            "script.callFunction",
            &json!({
                "functionDeclaration": function,
                "awaitPromise": false,
                "target": { "context": context },
                "arguments": [channel_argument],
                "resultOwnership": "none"
            }),
            event_tx,
        );
        match current_result {
            Ok(result) if result.get("type").and_then(Value::as_str) != Some("exception") => {}
            Ok(_) => {
                self.remove_firefox_network_bridge(event_tx);
                return Err(BrowserControlFailure::new(
                    "capture_protocol",
                    "Firefox page instrumentation failed in the active document",
                ));
            }
            Err(error) => {
                self.remove_firefox_network_bridge(event_tx);
                return Err(BrowserControlFailure::new("capture_protocol", error));
            }
        }
        Ok(())
    }

    pub(super) fn remove_firefox_network_bridge(&mut self, event_tx: &BrowserEventSender) {
        let Some(bridge) = self.firefox_network.as_ref() else {
            return;
        };
        let subscription = bridge.subscription.clone();
        let data_collector = bridge.data_collector.clone();
        if let Some(page) = bridge.page.as_ref() {
            let control_key = page.control_key.clone();
            let preload_script = page.preload_script.clone();
            if let Some(context) = self.context_id.clone() {
                let control_key = serde_json::to_string(&control_key).unwrap_or_else(|_| "\"\"".to_string());
                let function =
                    format!("() => {{ const control = globalThis[{control_key}]; if (control) control.stop(); }}");
                let _ = self.call_bidi(
                    "script.callFunction",
                    &json!({
                        "functionDeclaration": function,
                        "awaitPromise": false,
                        "target": { "context": context },
                        "resultOwnership": "none"
                    }),
                    event_tx,
                );
            }
            let _ = self.call_bidi(
                "script.removePreloadScript",
                &json!({ "script": preload_script }),
                event_tx,
            );
        }
        self.unsubscribe_network(&subscription, event_tx);
        if let Some(collector) = data_collector.as_deref() {
            self.remove_firefox_data_collector(collector, event_tx);
        }
        self.firefox_network = None;
    }

    fn remove_firefox_data_collector(&mut self, collector: &str, event_tx: &BrowserEventSender) {
        let _ = self.call_bidi(
            "network.removeDataCollector",
            &json!({ "collector": collector }),
            event_tx,
        );
    }

    fn unsubscribe_network(&mut self, subscription: &str, event_tx: &BrowserEventSender) {
        let _ = self.call_bidi(
            "session.unsubscribe",
            &json!({ "subscriptions": [subscription] }),
            event_tx,
        );
    }

    fn record_page_network_event(&mut self, record: &PageNetworkEvent) {
        match record.kind.as_str() {
            "created" => {
                if let Some(url) = record.url.as_deref() {
                    self.network.record_websocket_created(&record.connection_id, url);
                }
            }
            "opened" => self
                .network
                .record_websocket_opened(&record.connection_id, record.url.as_deref(), false),
            "frame" => {
                let (Some(direction), Some(encoding)) = (record.direction, record.encoding) else {
                    return;
                };
                self.network.record_websocket_frame(
                    &record.connection_id,
                    record.url.as_deref(),
                    direction,
                    record.opcode,
                    record.payload.as_deref(),
                    encoding,
                    record.payload_bytes,
                    record.truncated,
                );
            }
            "error" => self.network.record_websocket_terminal(
                &record.connection_id,
                BrowserNetworkEventKind::WebsocketError,
                record.error.as_deref(),
            ),
            "closed" => self.network.record_websocket_terminal(
                &record.connection_id,
                BrowserNetworkEventKind::WebsocketClosed,
                record.error.as_deref(),
            ),
            _ => {}
        }
    }
}

fn page_bridge_function(control_key: &str, options: &BrowserNetworkCaptureOptions) -> String {
    PAGE_BRIDGE_TEMPLATE
        .replace(
            "__CONTROL_KEY__",
            &serde_json::to_string(control_key).unwrap_or_else(|_| "\"\"".to_string()),
        )
        .replace(
            "__URL_PATTERNS__",
            &serde_json::to_string(&options.url_patterns).unwrap_or_else(|_| "[]".to_string()),
        )
        .replace(
            "__INCLUDE_SENT__",
            if options.frames.include_sent { "true" } else { "false" },
        )
        .replace(
            "__INCLUDE_RECEIVED__",
            if options.frames.include_received {
                "true"
            } else {
                "false"
            },
        )
        .replace("__MAX_PAYLOAD_BYTES__", &options.max_payload_bytes.to_string())
}

fn firefox_network_transport(options: &BrowserNetworkCaptureOptions) -> &'static str {
    if options.include_websocket {
        "webdriver_bidi_page_instrumentation"
    } else {
        "webdriver_bidi"
    }
}

fn firefox_network_events(options: &BrowserNetworkCaptureOptions) -> Vec<&'static str> {
    let mut events = Vec::new();
    if options.include_websocket {
        events.push("script.message");
    }
    if options.include_http {
        events.extend([
            "network.beforeRequestSent",
            "network.responseStarted",
            "network.responseCompleted",
            "network.fetchError",
        ]);
    }
    events
}

fn required_string(value: &Value, pointer: &str, label: &str) -> Result<String, BrowserControlFailure> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| BrowserControlFailure::new("capture_protocol", format!("Firefox omitted {label} identifier")))
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

fn u64_at(value: &Value, pointer: &str) -> Option<u64> {
    value.pointer(pointer).and_then(Value::as_u64)
}

fn u16_at(value: &Value, pointer: &str) -> Option<u16> {
    u64_at(value, pointer).and_then(|value| u16::try_from(value).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_bridge_embeds_bounded_capture_options() {
        let source = page_bridge_function(
            "capture-key",
            &BrowserNetworkCaptureOptions {
                frames: crate::BrowserNetworkFrameOptions {
                    include_sent: false,
                    ..crate::BrowserNetworkFrameOptions::default()
                },
                url_patterns: vec!["wss://example.test/market".to_string()],
                max_payload_bytes: 1234,
                ..BrowserNetworkCaptureOptions::default()
            },
        );

        assert!(source.contains("capture-key"));
        assert!(source.contains("wss://example.test/market"));
        assert!(source.contains("const includeSent = false"));
        assert!(source.contains("const maxPayloadBytes = 1234"));
        assert!(!source.contains("__MAX_PAYLOAD_BYTES__"));
    }

    #[test]
    fn http_only_capture_uses_bidi_without_page_instrumentation() {
        let options = BrowserNetworkCaptureOptions {
            include_http: true,
            include_websocket: false,
            ..BrowserNetworkCaptureOptions::default()
        };

        assert_eq!(firefox_network_transport(&options), "webdriver_bidi");
        assert!(!firefox_network_events(&options).contains(&"script.message"));
        assert!(firefox_network_events(&options).contains(&"network.responseCompleted"));
    }
}
