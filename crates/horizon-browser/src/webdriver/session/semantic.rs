//! Semantic DOM actions for Firefox and Safari `WebDriver` sessions.

use serde_json::{Value, json};

use crate::semantic::{
    bounded_control_value, check_script_error, parse_target_rect, scan_expression, scroll_expression,
    target_rect_expression, wait_scan_expression,
};
use crate::semantic_files::{
    attached_files_expression, check_attachment_request, file_input_probe_expression, local_file_facts,
    parse_attached_files, parse_file_input_probe, verify_attached,
};
use crate::semantic_fingerprint::{
    fingerprint_at_point_expression, fingerprint_focused_expression, fingerprint_from_script_value,
};
use crate::session::{BrowserEventSender, BrowserSessionConfig};
use crate::{
    AgentAction, BackendKind, BrowserButton, BrowserControlAction, BrowserControlFailure, BrowserControlValue,
    BrowserInput, BrowserModifiers, BrowserSnapshot,
};

use super::super::transport::{ClassicTransport, encode_path_segment};
use super::{Driver, create_webdriver_session, webdriver_value};

pub(super) fn create_webdriver_session_response(
    transport: &dyn ClassicTransport,
    config: &BrowserSessionConfig,
) -> Result<Value, String> {
    match create_webdriver_session(transport, config, true) {
        Ok(response) => Ok(response),
        Err(error)
            if config.browser.backend == BackendKind::SafariWebDriver
                && error.is_unsupported_websocket_capability() =>
        {
            create_webdriver_session(transport, config, false)
                .map_err(|error| format!("failed to create classic Safari WebDriver session: {error}"))
        }
        Err(error) => Err(format!("failed to create WebDriver session: {error}")),
    }
}

/// The W3C element identifier key in a Find Element response.
const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

/// The element reference in a Find Element response, accepting the W3C key
/// and the legacy `ELEMENT` key some grids still send.
fn element_reference(response: &Value) -> Option<&str> {
    let value = webdriver_value(response)?;
    value
        .get(ELEMENT_KEY)
        .or_else(|| value.get("ELEMENT"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// Replace the text of the element `selector` matches through the W3C Find
/// Element, Element Clear and Element Send Keys commands under `session`
/// (`/session/<id>`). The opaque element reference becomes one encoded route
/// segment, and the first failing command ends the sequence. Success also
/// requires the live field to retain the requested value after native input.
fn send_keys_through(
    transport: &dyn ClassicTransport,
    session: &str,
    selector: &str,
    text: &str,
) -> Result<(), String> {
    let post = |suffix: &str, body: &Value| {
        transport
            .post(&format!("{session}/{suffix}"), body)
            .map_err(|error| error.to_string())
    };
    let element = find_element_segment(&post, selector)?;
    post(&format!("element/{element}/clear"), &json!({}))?;
    if !text.is_empty() {
        post(&format!("element/{element}/value"), &json!({ "text": text }))?;
    }
    // Resolve again: a reactive handler may replace the original element.
    // Return only equality, never the current or requested field contents.
    let response = post(
        "execute/sync",
        &json!({
            "script": "const element = document.querySelector(arguments[0]); return !!element && (element.isContentEditable ? element.textContent : element.value) === arguments[1];",
            "args": [selector, text],
        }),
    )?;
    if webdriver_value(&response).and_then(Value::as_bool) != Some(true) {
        return Err("remote fill did not retain the requested value; native input may be unsupported or the page may have changed the field".to_string());
    }
    Ok(())
}

/// Attach host files to the file input `selector` matches through the W3C
/// Find Element and Element Send Keys commands under `session`: for an
/// `input[type=file]`, Send Keys takes newline-separated host paths instead
/// of typing them. The first failing command ends the sequence.
fn set_files_through(
    transport: &dyn ClassicTransport,
    session: &str,
    selector: &str,
    paths: &[std::path::PathBuf],
) -> Result<(), String> {
    let post = |suffix: &str, body: &Value| {
        transport
            .post(&format!("{session}/{suffix}"), body)
            .map_err(|error| error.to_string())
    };
    let text = paths
        .iter()
        .map(|path| {
            let text = path.to_string_lossy();
            if text.contains(['\n', '\r']) {
                // The separator is the newline; a path carrying one would
                // split into other uploads.
                return Err("attachment path contains a line break".to_string());
            }
            Ok(text.into_owned())
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    let element = find_element_segment(&post, selector)?;
    post(&format!("element/{element}/value"), &json!({ "text": text }))?;
    Ok(())
}

/// Click the element `selector` matches through the W3C Find Element and
/// Element Click commands under `session`, so the driver, not a pointer
/// action aimed at a viewport coordinate, decides where the tap lands. On
/// a real iOS device a pointer action at the element's page rectangle can
/// hit the element above it (the Safari toolbar shifts the mapping); the
/// driver's own element click does not.
fn click_through(transport: &dyn ClassicTransport, session: &str, selector: &str) -> Result<(), String> {
    let post = |suffix: &str, body: &Value| {
        transport
            .post(&format!("{session}/{suffix}"), body)
            .map_err(|error| error.to_string())
    };
    let element = find_element_segment(&post, selector)?;
    // Element Click may wait for a navigation the element triggers, so it
    // gets the navigation-sized read timeout rather than the command default.
    transport
        .post_with_read_timeout(
            &format!("{session}/element/{element}/click"),
            &json!({}),
            super::NAVIGATION_HTTP_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Find Element by CSS selector, returning the reference as one encoded
/// route segment; a reference that cannot form a route is refused here
/// rather than on the wire.
fn find_element_segment(
    post: &dyn Fn(&str, &Value) -> Result<Value, String>,
    selector: &str,
) -> Result<String, String> {
    let found = post("element", &json!({ "using": "css selector", "value": selector }))?;
    let element = element_reference(&found).ok_or_else(|| "WebDriver returned no element reference".to_string())?;
    if element.contains(['/', '%', '\\']) || element == "." || element == ".." {
        // A separator, a percent sign or a dot-only reference cannot be one
        // route segment (the transport refuses each, since a decoding proxy
        // could reshape the route); say why instead of failing on the wire.
        return Err(format!(
            "WebDriver returned an element reference that cannot form a route ({} bytes)",
            element.len()
        ));
    }
    Ok(encode_path_segment(element))
}

const DOCUMENT_IDENTITY_EXPRESSION: &str =
    "JSON.stringify([String(location.href), Number(globalThis.performance?.timeOrigin || 0)])";

impl Driver {
    pub(super) fn execute_agent_action(
        &mut self,
        request: &AgentAction,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        if self.panel_slot.teach_recording()
            && !matches!(
                &request.action,
                BrowserControlAction::Snapshot { .. } | BrowserControlAction::Query { .. }
            )
        {
            return Err(BrowserControlFailure::new(
                "teach_recording",
                "Teach mode has exclusive ownership of this panel",
            ));
        }
        if let Some(command) = request.action.to_command() {
            let failure_code = if matches!(command, crate::session::BrowserCommand::Input(_)) {
                "input_failed"
            } else {
                "navigation_failed"
            };
            return self
                .run_command(command, event_tx, false)
                .map(|_| BrowserControlValue::Accepted)
                .map_err(|error| BrowserControlFailure::new(failure_code, error));
        }
        match &request.action {
            BrowserControlAction::Resize { .. } => Err(BrowserControlFailure::new(
                "invalid_action_state",
                "resize is observed from the driver loop",
            )),
            BrowserControlAction::Snapshot { max_nodes } => self.semantic_snapshot(*max_nodes),
            BrowserControlAction::Query { selector, max_results } => self.semantic_query(selector, *max_results),
            BrowserControlAction::WaitForSelector { .. } => Err(BrowserControlFailure::new(
                "invalid_action_state",
                "selector waits are observed from the driver loop",
            )),
            BrowserControlAction::Click { target, count } => self.semantic_click(target, *count, event_tx),
            BrowserControlAction::Fill { target, value } => self.semantic_fill(target, value, event_tx),
            BrowserControlAction::Scroll {
                target,
                delta_x,
                delta_y,
            } => self.semantic_scroll(target.as_ref(), *delta_x, *delta_y),
            BrowserControlAction::SetFiles { target, paths } => self.semantic_set_files(target, paths),
            BrowserControlAction::Evaluate { expression } => self.semantic_evaluate(expression),
            BrowserControlAction::Network { operation, options } => {
                self.network_action(request, *operation, options.clone(), event_tx)
            }
            BrowserControlAction::Video { operation, options } => {
                self.video_action(&request.action_id, *operation, options.as_ref())
            }
            BrowserControlAction::HttpAuth { .. } => self.http_auth_action(&request.action),
            BrowserControlAction::Navigate { .. }
            | BrowserControlAction::Reload
            | BrowserControlAction::Back
            | BrowserControlAction::Forward
            | BrowserControlAction::Input { .. } => Err(BrowserControlFailure::new(
                "invalid_action_state",
                "command action was not dispatched",
            )),
        }
    }

    fn semantic_snapshot(&mut self, max_nodes: u32) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(&scan_expression(None, max_nodes))?;
        let (generation, revision, nodes) = self.semantic.register_nodes(value)?;
        Ok(BrowserControlValue::Snapshot {
            snapshot: BrowserSnapshot {
                generation,
                revision,
                url: self.url.clone(),
                title: self.title.clone(),
                nodes,
            },
        })
    }

    pub(super) fn semantic_query(
        &mut self,
        selector: &str,
        max_results: u32,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(&scan_expression(Some(selector), max_results))?;
        self.register_query(value)
    }

    /// A selector scan that does not register references: for judging a wait
    /// condition without disturbing refs a concurrent snapshot handed out.
    pub(super) fn semantic_peek_within(
        &mut self,
        selector: &str,
        max_results: u32,
        timeout: std::time::Duration,
    ) -> Result<
        (
            u64,
            Vec<crate::BrowserNode>,
            Option<crate::semantic::ScanSummary>,
            Value,
        ),
        BrowserControlFailure,
    > {
        let value = self.evaluate_json_within(&wait_scan_expression(selector, max_results), Some(timeout))?;
        let peeked = self.semantic.peek_nodes(&value)?;
        self.record_classic_document_identity(peeked.document_identity);
        Ok((self.semantic.generation(), peeked.nodes, peeked.summary, value))
    }

    /// Register a previously peeked scan as the current references.
    pub(super) fn semantic_register_scan(
        &mut self,
        value: Value,
    ) -> Result<(u64, u64, Vec<crate::BrowserNode>), BrowserControlFailure> {
        self.semantic.register_nodes(value)
    }

    fn register_query(&mut self, value: Value) -> Result<BrowserControlValue, BrowserControlFailure> {
        let (generation, revision, nodes) = self.semantic.register_nodes(value)?;
        Ok(BrowserControlValue::Nodes {
            generation,
            revision,
            nodes,
        })
    }

    fn semantic_click(
        &mut self,
        target: &crate::BrowserTarget,
        count: u32,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = self.semantic.resolve(target)?;
        if self.remote_android_chromium && count == 1 {
            let (x, y) = self.remote_click_point(&selector)?;
            self.perform_click(x, y, count, event_tx)
                .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
            return Ok(BrowserControlValue::Accepted);
        }
        let value = self.evaluate_json(&target_rect_expression(&selector, false))?;
        let (x, y) = parse_target_rect(&value)?;
        self.capture_teach_fingerprint(Some((x, y)))?;
        if self.host.is_remote() && count == 1 {
            // A pointer action at (x, y) landed on the element above the
            // target on a real iPhone (2026-09-14 live run: the submit
            // click raised the keyboard for the field instead); Element
            // Click lets the remote driver place the tap itself.
            self.pending_classic_history_start = None;
            let result = click_through(
                self.host.transport(),
                &format!("/session/{}", self.session_id),
                &selector,
            );
            if !self.retain_frame_during_navigation {
                self.scrollbar.refresh_at = std::time::Instant::now();
                self.frames.demand();
            }
            result.map_err(|error| BrowserControlFailure::new("input_failed", error))?;
            return Ok(BrowserControlValue::Accepted);
        }
        self.perform_click(x, y, count, event_tx)
            .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        Ok(BrowserControlValue::Accepted)
    }

    fn perform_click(&mut self, x: f64, y: f64, count: u32, event_tx: &BrowserEventSender) -> Result<(), String> {
        self.pending_classic_history_start = None;
        if self.safari.is_some() {
            return self.perform_safari_click(x, y, count, event_tx);
        }
        let mut payload = self
            .actions
            .click_payload(x, y, BrowserButton::Left, count, BrowserModifiers::none());
        if self.remote_android_chromium && count == 1 {
            super::remote_click::use_touch_pointer(&mut payload);
        }
        let result = if self.firefox_bidi() {
            payload["context"] = json!(self.context_id);
            self.call_bidi("input.performActions", &payload, event_tx).map(|_| ())
        } else if self.remote_android_chromium && count == 1 {
            super::remote_click::click_through(
                self.host.transport(),
                &format!("/session/{}", self.session_id),
                &payload,
            )
        } else {
            self.classic_post("actions", &payload).map(|_| ())
        };
        if let Err(error) = &result {
            tracing::warn!("WebDriver input failed: {error}");
        }
        if !self.retain_frame_during_navigation {
            self.scrollbar.refresh_at = std::time::Instant::now();
            self.frames.demand();
        }
        result
    }

    fn semantic_fill(
        &mut self,
        target: &crate::BrowserTarget,
        value: &str,
        event_tx: &BrowserEventSender,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = self.semantic.resolve(target)?;
        let result = self.evaluate_json(&target_rect_expression(&selector, !self.host.is_remote()))?;
        let _ = parse_target_rect(&result)?;
        self.capture_teach_fingerprint(None)?;
        if self.host.is_remote() {
            // Key actions to the focused field do not reliably produce text
            // on a real device (iOS Safari left the field empty in the
            // 2026-09-14 live run); Element Send Keys is the text-entry path
            // every remote grid implements, so a fill goes through it.
            // The page may already have changed (the field was cleared)
            // even when a later command fails, so a frame is demanded
            // either way, as perform_input does.
            let result = self.classic_send_keys(&selector, value);
            self.frames.demand();
            result.map_err(|error| BrowserControlFailure::new("input_failed", error))?;
            return Ok(BrowserControlValue::Accepted);
        }
        self.perform_input(
            BrowserInput::InsertText {
                text: value.to_string(),
            },
            event_tx,
        )
        .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        if self.safari.is_some() {
            self.flush_safari_input(event_tx)
                .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        }
        Ok(BrowserControlValue::Accepted)
    }

    /// Attach host files through Element Send Keys on the file input. A
    /// remote device runs on another host, where the paths mean nothing and
    /// no file transfer exists, so it is refused as unsupported rather than
    /// attempted.
    fn semantic_set_files(
        &mut self,
        target: &crate::BrowserTarget,
        paths: &[std::path::PathBuf],
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        if self.host.is_remote() {
            return Err(BrowserControlFailure::new(
                "unsupported_backend",
                "file attachment is unavailable for remote device sessions: the files live on this host and no transfer to the remote browser exists",
            ));
        }
        let selector = self.semantic.resolve(target)?;
        let expected = local_file_facts(paths)?;
        let probe = self.evaluate_json(&file_input_probe_expression(&selector))?;
        check_attachment_request(&parse_file_input_probe(&probe)?, paths)?;
        self.capture_teach_fingerprint(None)?;
        let result = set_files_through(
            self.host.transport(),
            &format!("/session/{}", self.session_id),
            &selector,
            paths,
        );
        self.frames.demand();
        result.map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        let readback = self.evaluate_json(&attached_files_expression(&selector))?;
        let attached = parse_attached_files(&readback)?;
        verify_attached(&attached, &expected)?;
        Ok(BrowserControlValue::Files { files: attached })
    }

    fn classic_send_keys(&self, selector: &str, text: &str) -> Result<(), String> {
        send_keys_through(
            self.host.transport(),
            &format!("/session/{}", self.session_id),
            selector,
            text,
        )
    }

    fn semantic_scroll(
        &mut self,
        target: Option<&crate::BrowserTarget>,
        delta_x: f64,
        delta_y: f64,
    ) -> Result<BrowserControlValue, BrowserControlFailure> {
        let selector = target.map(|target| self.semantic.resolve(target)).transpose()?;
        let value = self.evaluate_json(&scroll_expression(selector.as_deref(), delta_x, delta_y))?;
        check_script_error(&value)?;
        self.frames.demand();
        Ok(BrowserControlValue::Json { value })
    }

    fn semantic_evaluate(&self, expression: &str) -> Result<BrowserControlValue, BrowserControlFailure> {
        let value = self.evaluate_json(expression)?;
        Ok(BrowserControlValue::Json { value })
    }

    pub(super) fn capture_teach_input(&mut self, input: &BrowserInput) {
        if !self.panel_slot.teach_recording() {
            return;
        }
        let Some(capture) = self.semantic.teach_capture_point(input) else {
            return;
        };
        if let Err(error) = self.capture_teach_fingerprint(capture.point()) {
            tracing::warn!(target: "browser", "teach fingerprint failed: {}", error.message);
        }
    }

    pub(super) fn capture_teach_fingerprint(&mut self, point: Option<(f64, f64)>) -> Result<(), BrowserControlFailure> {
        if !self.panel_slot.teach_recording() {
            return Ok(());
        }
        let generation = self.panel_slot.teach_generation();
        let Some((x, y)) = point else {
            let _ = fingerprint_focused_expression();
            return Ok(());
        };
        let expression = fingerprint_at_point_expression(x, y);
        let value = match self.evaluate_json(&expression) {
            Ok(value) => value,
            Err(error) => {
                self.panel_slot
                    .store_teach_failure(&error.code, &error.message, generation);
                return Err(error);
            }
        };
        let fingerprint = match fingerprint_from_script_value(&value) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                self.panel_slot
                    .store_teach_failure(&error.code, &error.message, generation);
                return Err(error);
            }
        };
        self.panel_slot.store_teach_fingerprint(fingerprint, generation);
        Ok(())
    }

    fn evaluate_json(&self, expression: &str) -> Result<Value, BrowserControlFailure> {
        self.evaluate_json_within(expression, None)
    }

    pub(super) fn evaluate_json_within(
        &self,
        expression: &str,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, BrowserControlFailure> {
        let body = json!({ "script": format!("return ({expression});"), "args": [] });
        let response = match timeout {
            Some(timeout) => self.classic_navigation_post_within("execute/sync", &body, timeout),
            None => self.classic_post("execute/sync", &body),
        }
        .map_err(|error| BrowserControlFailure::new("javascript_error", error))?;
        let value = webdriver_value(&response)
            .cloned()
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no script value"))?;
        bounded_control_value(value)
    }

    /// Classic `WebDriver` has no navigation event stream, so a session that
    /// runs on it alone tracks the document identity by script: Safari
    /// locally, and every remote session whatever browser it drives.
    fn tracks_classic_document_identity(&self) -> bool {
        self.config.browser.backend == BackendKind::SafariWebDriver || self.host.is_remote()
    }

    pub(super) fn initialize_classic_document_identity(&mut self) {
        if self.tracks_classic_document_identity() {
            let _ = self.refresh_classic_document_identity_within(std::time::Duration::from_secs(1));
        }
    }

    pub(super) fn refresh_classic_document_identity_within(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<bool, BrowserControlFailure> {
        if !self.tracks_classic_document_identity() {
            return Ok(false);
        }
        let value = self.evaluate_json_within(DOCUMENT_IDENTITY_EXPRESSION, Some(timeout))?;
        let identity = value
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "WebDriver returned no document identity"))?;
        Ok(self.record_classic_document_identity(Some(identity)))
    }

    fn record_classic_document_identity(&mut self, identity: Option<String>) -> bool {
        if !self.tracks_classic_document_identity() {
            return false;
        }
        let Some(identity) = identity else {
            return false;
        };
        let changed = self
            .classic_document_identity
            .replace(identity.clone())
            .is_some_and(|previous| previous != identity);
        if changed {
            self.semantic.invalidate();
            self.advance_generation();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::time::Duration;

    use serde_json::{Value, json};

    use super::super::super::http::HttpError;
    use super::super::super::transport::ClassicTransport;
    use super::{click_through, element_reference, send_keys_through, set_files_through};

    /// Answers each command with the next scripted reply and records what
    /// was sent.
    struct Scripted {
        replies: Mutex<Vec<Result<Value, String>>>,
        sent: Mutex<Vec<(String, String, Value)>>,
        timeouts: Mutex<Vec<Duration>>,
    }

    impl Scripted {
        fn new(replies: Vec<Result<Value, String>>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().rev().collect()),
                sent: Mutex::new(Vec::new()),
                timeouts: Mutex::new(Vec::new()),
            }
        }

        fn timeouts(&self) -> Vec<Duration> {
            self.timeouts.lock().expect("timeouts").clone()
        }

        fn sent(&self) -> Vec<(String, String, Value)> {
            self.sent.lock().expect("sent").clone()
        }
    }

    impl ClassicTransport for Scripted {
        fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<&Value>,
            read_timeout: Duration,
        ) -> Result<Value, HttpError> {
            self.timeouts.lock().expect("timeouts").push(read_timeout);
            self.sent.lock().expect("sent").push((
                method.to_string(),
                path.to_string(),
                body.cloned().unwrap_or(Value::Null),
            ));
            match self.replies.lock().expect("replies").pop() {
                Some(Ok(value)) => Ok(value),
                Some(Err(message)) => Err(HttpError::InvalidResponse(message)),
                None => Err(HttpError::InvalidResponse("unscripted command".into())),
            }
        }
    }

    #[test]
    fn a_fill_finds_clears_and_sends_keys_to_one_element() {
        let transport = Scripted::new(vec![
            Ok(json!({"value": {"element-6066-11e4-a52e-4f735466cecf": "node 1 {a}"}})),
            Ok(json!({"value": null})),
            Ok(json!({"value": null})),
            Ok(json!({"value": true})),
        ]);
        send_keys_through(&transport, "/session/s1", "input[name=q]", "Ada").expect("fill");
        assert_eq!(
            transport.sent()[..3],
            vec![
                (
                    "POST".to_string(),
                    "/session/s1/element".to_string(),
                    json!({"using": "css selector", "value": "input[name=q]"})
                ),
                (
                    "POST".to_string(),
                    "/session/s1/element/node%201%20%7Ba%7D/clear".to_string(),
                    json!({})
                ),
                (
                    "POST".to_string(),
                    "/session/s1/element/node%201%20%7Ba%7D/value".to_string(),
                    json!({"text": "Ada"})
                ),
            ]
        );
    }

    mod fill;
    mod set_files;

    #[test]
    fn a_click_finds_the_element_and_clicks_it_through_the_driver() {
        let transport = Scripted::new(vec![
            Ok(json!({"value": {"element-6066-11e4-a52e-4f735466cecf": "node 7"}})),
            Ok(json!({"value": null})),
        ]);
        click_through(&transport, "/session/s1", "#submit").expect("click");
        assert_eq!(
            transport.sent(),
            vec![
                (
                    "POST".to_string(),
                    "/session/s1/element".to_string(),
                    json!({"using": "css selector", "value": "#submit"})
                ),
                (
                    "POST".to_string(),
                    "/session/s1/element/node%207/click".to_string(),
                    json!({})
                ),
            ]
        );
        assert_eq!(
            transport.timeouts(),
            vec![
                super::super::super::transport::DEFAULT_READ_TIMEOUT,
                super::super::NAVIGATION_HTTP_TIMEOUT
            ],
            "the click waits as long as a navigation may take"
        );

        let transport = Scripted::new(vec![Err("no such element".into())]);
        let error = click_through(&transport, "/session/s1", "#missing").expect_err("not found");
        assert!(error.contains("no such element"), "{error}");
        assert_eq!(transport.sent().len(), 1, "nothing follows a failed Find Element");

        let transport = Scripted::new(vec![Ok(json!({"value": {"ELEMENT": "a/b"}}))]);
        let error = click_through(&transport, "/session/s1", "#x").expect_err("unroutable");
        assert!(error.contains("cannot form a route"), "{error}");
        assert_eq!(transport.sent().len(), 1);
    }

    #[test]
    fn native_mobile_click_preserves_the_element_click_navigation_timeout() {
        let transport = Scripted::new(vec![Ok(json!({"value": null}))]);
        let payload = json!({"actions":[{"type":"pointer","id":"horizon-touch",
        "parameters":{"pointerType":"touch"},"actions":[
            {"type":"pointerMove","origin":"viewport","x":80,"y":38},
            {"type":"pointerDown","button":0},{"type":"pointerUp","button":0}
        ]}]});
        super::super::remote_click::click_through(&transport, "/session/s1", &payload).expect("native tap");
        assert_eq!(
            transport.sent(),
            vec![("POST".into(), "/session/s1/actions".into(), payload)]
        );
        assert_eq!(transport.timeouts(), vec![super::super::NAVIGATION_HTTP_TIMEOUT]);
    }

    #[test]
    fn a_failing_command_ends_the_fill_sequence() {
        let transport = Scripted::new(vec![Ok(json!({"value": {"ELEMENT": "e1"}})), Err("stale".into())]);
        let error = send_keys_through(&transport, "/session/s1", "#name", "x").expect_err("clear failed");
        assert!(error.contains("stale"), "{error}");
        let sent = transport.sent();
        assert_eq!(sent.len(), 2, "no Send Keys after a failed Clear: {sent:?}");
        assert_eq!(sent[1].1, "/session/s1/element/e1/clear");

        let transport = Scripted::new(vec![Ok(json!({"value": {}}))]);
        let error = send_keys_through(&transport, "/session/s1", "#name", "x").expect_err("no reference");
        assert!(error.contains("no element reference"), "{error}");
        assert_eq!(transport.sent().len(), 1, "nothing follows a missing reference");

        for reference in ["a/b", "a%2Fb", "a\\b", "..", "."] {
            let transport = Scripted::new(vec![Ok(json!({"value": {"ELEMENT": reference}}))]);
            let error = send_keys_through(&transport, "/session/s1", "#name", "x").expect_err("unroutable");
            assert!(error.contains("cannot form a route"), "{error}");
            assert_eq!(
                transport.sent().len(),
                1,
                "a reference that cannot form a route is never sent"
            );
        }
    }

    #[test]
    fn element_references_accept_the_w3c_and_legacy_keys() {
        assert_eq!(
            element_reference(&json!({"value": {"element-6066-11e4-a52e-4f735466cecf": "e1"}})),
            Some("e1")
        );
        assert_eq!(
            element_reference(&json!({"value": {"ELEMENT": "legacy"}})),
            Some("legacy")
        );
        assert_eq!(
            element_reference(&json!({"value": {"element-6066-11e4-a52e-4f735466cecf": ""}})),
            None
        );
        assert_eq!(element_reference(&json!({"value": null})), None);
    }
}
