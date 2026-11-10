//! The browser metadata that the disclosure policy needs before a page runs.
//!
//! Chromium reports a `HeadlessChrome` user agent over a `DevTools` link, which a
//! page can read before any script of this engine runs. A few CDP calls remove
//! that signal for the page of this session, and one of them needs the browser's
//! own Client Hints values, which only a document of that browser can read. This
//! module reads them from a hidden target of the same browser and closes it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::cdp::CdpLink;
use crate::disclosure::{
    CHROMIUM_DISCLOSURE_BOOTSTRAP_URL, CHROMIUM_USER_AGENT_METADATA_EXPRESSION, chromium_user_agent_needs_override,
};

use crate::AutomationDisclosurePolicy;

use super::super::shared::DriverProcess;
use super::{CALL_TIMEOUT, call_during_startup, call_in_target};

/// Poll interval while the disclosure target loads its bootstrap page.
const DISCLOSURE_DOCUMENT_POLL: Duration = Duration::from_millis(20);

pub(super) fn prepare_disclosure_metadata(
    link: &mut CdpLink,
    chrome: &DriverProcess,
    stop_requested: &AtomicBool,
    policy: AutomationDisclosurePolicy,
) -> Result<Option<serde_json::Value>, String> {
    if policy == AutomationDisclosurePolicy::BrowserDefault {
        return Ok(None);
    }
    let version = call_during_startup(link, stop_requested, "Browser.getVersion", &serde_json::json!({}))
        .map_err(|error| format!("Browser.getVersion: {error}"))?;
    if !chromium_user_agent_needs_override(&version).map_err(str::to_string)? {
        return Ok(None);
    }
    let created = call_during_startup(
        link,
        stop_requested,
        "Target.createTarget",
        &serde_json::json!({
            "url": CHROMIUM_DISCLOSURE_BOOTSTRAP_URL,
            "background": true,
        }),
    )
    .map_err(|error| format!("Target.createTarget: {error}"))?;
    let target_id = created
        .get("targetId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Target.createTarget omitted targetId".to_string())?
        .to_string();

    chrome.register_target(&target_id);
    let metadata = read_disclosure_metadata_from_target(link, stop_requested, &target_id);
    let close_result = link
        .call_and_drain(
            CALL_TIMEOUT,
            "Target.closeTarget",
            &serde_json::json!({ "targetId": target_id }),
            None,
        )
        .result
        .map_err(|error| format!("Target.closeTarget: {error}"))
        .and_then(|result| {
            result
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .is_some_and(|success| success)
                .then_some(())
                .ok_or_else(|| "Target.closeTarget did not close the disclosure target".to_string())
        });
    if close_result.is_ok() {
        chrome.forget_closed_target(&target_id);
    }
    match (metadata, close_result) {
        (Ok(metadata), Ok(())) => Ok(Some(metadata)),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

pub(super) fn read_disclosure_metadata_from_target(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    target_id: &str,
) -> Result<serde_json::Value, String> {
    let attached = call_during_startup(
        link,
        stop_requested,
        "Target.attachToTarget",
        &serde_json::json!({ "targetId": target_id, "flatten": true }),
    )
    .map_err(|error| format!("Target.attachToTarget: {error}"))?;
    let session_id = attached
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Target.attachToTarget omitted sessionId".to_string())?;
    call_in_target(
        link,
        stop_requested,
        session_id,
        CALL_TIMEOUT,
        "Runtime.enable",
        &serde_json::json!({}),
    )?;
    wait_for_disclosure_document(link, stop_requested, session_id, CALL_TIMEOUT)?;
    call_in_target(
        link,
        stop_requested,
        session_id,
        CALL_TIMEOUT,
        "Runtime.evaluate",
        &serde_json::json!({
            "expression": CHROMIUM_USER_AGENT_METADATA_EXPRESSION,
            "awaitPromise": true,
            "returnByValue": true,
        }),
    )
}

/// `Target.createTarget` can answer before the new target commits its URL. Until then the
/// target shows its initial `about:blank` document, which is not a secure context, so
/// `navigator.userAgentData` is absent there. Read the metadata only from the bootstrap page.
pub(super) fn wait_for_disclosure_document(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    session_id: &str,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut shown = "no document URL".to_string();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "the disclosure target did not show {CHROMIUM_DISCLOSURE_BOOTSTRAP_URL}; last result: {shown}"
            ));
        }
        // Each read gets only the remaining time, so the wait ends at the deadline.
        let location = call_in_target(
            link,
            stop_requested,
            session_id,
            remaining.min(CALL_TIMEOUT),
            "Runtime.evaluate",
            &serde_json::json!({ "expression": "location.href", "returnByValue": true }),
        );
        shown = match location {
            Ok(result) => result
                .pointer("/result/value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("no document URL")
                .to_string(),
            Err(error) => error,
        };
        if shown == CHROMIUM_DISCLOSURE_BOOTSTRAP_URL {
            return Ok(());
        }
        if stop_requested.load(Ordering::Acquire) {
            return Err("browser startup was cancelled".to_string());
        }
        std::thread::sleep(DISCLOSURE_DOCUMENT_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}
