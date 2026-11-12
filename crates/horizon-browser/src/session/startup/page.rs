//! Which page of the browser this session drives.
//!
//! A browser of another process already holds the pages of its person, and a
//! panel that lost its link asks for the page it had before it opens a new one.

use std::sync::atomic::AtomicBool;

use crate::cdp::CdpLink;

use super::super::shared::DriverProcess;
use super::{BrowserSessionConfig, call_during_startup};

/// The page this session drives: the one the panel had before a lost link when
/// it still exists, otherwise the page this browser already gave this process
/// or a new page.
pub(crate) fn caller_page(
    config: &BrowserSessionConfig,
    chrome: &DriverProcess,
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
) -> (bool, Option<String>) {
    let resume_target = config.resume_target.as_deref().filter(|_| chrome.is_attached());
    let resumed_target = resume_target.and_then(|target| existing_page_target(link, stop_requested, target));
    if let (Some(wanted), None) = (resume_target, resumed_target.as_ref()) {
        tracing::info!(
            target: "browser",
            target_id = wanted,
            "the page of the lost link is gone, so this session opens the last address in a new page"
        );
    }
    if let Some(target) = resumed_target {
        return (true, Some(target));
    }
    if chrome.is_shared() || chrome.is_attached() {
        return (false, chrome.registered_target());
    }
    (false, first_page_target(link, stop_requested))
}

/// The page target `wanted` of the browser, when it still exists as a page.
///
/// This is the question that decides a reattach: a link that comes back to a
/// browser that kept running finds the old page and keeps its state; a browser
/// that restarted has no such target, and the caller opens a new page of the
/// last address.
fn existing_page_target(link: &mut CdpLink, stop_requested: &AtomicBool, wanted: &str) -> Option<String> {
    let result = call_during_startup(link, stop_requested, "Target.getTargets", &serde_json::json!({})).ok()?;
    result
        .get("targetInfos")
        .and_then(|targets| targets.as_array())
        .and_then(|targets| {
            targets.iter().find(|target| {
                target.get("targetId").and_then(serde_json::Value::as_str) == Some(wanted)
                    && target.get("type").and_then(serde_json::Value::as_str) == Some("page")
            })
        })
        .and_then(|target| target.get("targetId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// First existing `page` target, if any.
pub(crate) fn first_page_target(link: &mut CdpLink, stop_requested: &AtomicBool) -> Option<String> {
    let result = call_during_startup(link, stop_requested, "Target.getTargets", &serde_json::json!({})).ok()?;
    result
        .get("targetInfos")
        .and_then(|t| t.as_array())
        .and_then(|targets| first_available_page_target_id(targets))
        .map(str::to_string)
}

pub(super) fn first_available_page_target_id(targets: &[serde_json::Value]) -> Option<&str> {
    targets
        .iter()
        .find(|target| {
            target.get("type").and_then(serde_json::Value::as_str) == Some("page")
                && !target
                    .get("attached")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        })
        .and_then(|target| target.get("targetId"))
        .and_then(serde_json::Value::as_str)
}

/// Create a fresh page target as a fallback (browser opened without one).
/// `background` opens it in its own window that does not take the focus.
pub(crate) fn create_page_target(link: &mut CdpLink, stop_requested: &AtomicBool, background: bool) -> Option<String> {
    let params = if background {
        serde_json::json!({ "url": "about:blank", "newWindow": true, "background": true })
    } else {
        serde_json::json!({ "url": "about:blank" })
    };
    let result = call_during_startup(link, stop_requested, "Target.createTarget", &params).ok()?;
    result.get("targetId").and_then(|t| t.as_str()).map(str::to_string)
}
