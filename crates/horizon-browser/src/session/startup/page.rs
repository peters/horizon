//! Which page of the browser this session drives.
//!
//! A browser of another process already holds the pages of its person, and a
//! panel that lost its link asks for the page it had before it opens a new one.

use std::sync::atomic::AtomicBool;

use crate::cdp::CdpLink;

use super::super::shared::DriverProcess;
use super::{BrowserSessionConfig, PageIsolation, call_during_startup};

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
    let resumed_target = resume_target.and_then(|target| existing_page_target(link, stop_requested, target, None));
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

/// The page target `wanted` of the browser, when it still exists as a page of
/// `in_context`, or as a page of any context when that is `None`.
///
/// This is the question that decides a reattach: a link that comes back to a
/// browser that kept running finds the old page and keeps its state; a browser
/// that restarted has no such target, and the caller opens a new page of the
/// last address. A private page asks this question with its own context, so a
/// page of the default context of that browser is never its page.
fn existing_page_target(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    wanted: &str,
    in_context: Option<&str>,
) -> Option<String> {
    let result = call_during_startup(link, stop_requested, "Target.getTargets", &serde_json::json!({})).ok()?;
    result
        .get("targetInfos")
        .and_then(|targets| targets.as_array())
        .and_then(|targets| {
            targets.iter().find(|target| {
                target.get("targetId").and_then(serde_json::Value::as_str) == Some(wanted)
                    && target.get("type").and_then(serde_json::Value::as_str) == Some("page")
                    && in_context.is_none_or(|context| {
                        target.get("browserContextId").and_then(serde_json::Value::as_str) == Some(context)
                    })
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
    create_page_target_in(link, stop_requested, background, None)
}

/// Create a page inside `context`, the isolated browser context of a private
/// page, or in the default context of the browser when it is `None`.
///
/// A page of an isolated context sees none of the cookies of the profile, and
/// keeps none of its own after the context is disposed.
pub(crate) fn create_page_target_in(
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
    background: bool,
    context: Option<&str>,
) -> Option<String> {
    let mut params = if background {
        serde_json::json!({ "url": "about:blank", "newWindow": true, "background": true })
    } else {
        serde_json::json!({ "url": "about:blank" })
    };
    if let Some(context) = context {
        params["browserContextId"] = serde_json::Value::from(context);
    }
    let result = call_during_startup(link, stop_requested, "Target.createTarget", &params).ok()?;
    result.get("targetId").and_then(|t| t.as_str()).map(str::to_string)
}

/// The start of the page that a session drives, and the fact that it proves.
pub(crate) struct PageStart {
    /// The page of the link before, when this link still holds it.
    pub(crate) target: Option<String>,
    /// Whether `target` is the page of the link before.
    pub(crate) resumed: bool,
    /// The isolated browser context for the page that this session makes. A page
    /// of the default context leaves it `None`.
    pub(crate) context: Option<String>,
    /// What this session established about its page.
    pub(crate) isolation: PageIsolation,
    /// Why a private page could not be made, when it could not.
    pub(crate) refusal: Option<String>,
}

impl PageStart {
    fn refused(detail: String) -> Self {
        Self {
            target: None,
            resumed: false,
            context: None,
            isolation: PageIsolation::Shared,
            refusal: Some(detail),
        }
    }
}

/// The start of this session's page: the page of the link before when the
/// browser still holds it, the isolated context when the panel asked for a
/// private page, and the fact that the panel reads.
///
/// A private page is a page of an isolated browser context of the browser that
/// another process owns, and never a page of the default context: the page of the
/// link before comes back only inside the context that it had, and a browser
/// whose context is gone gets a new context and a new page. A lane that cannot
/// make one refuses, so no page of the shared profile is opened in its place.
pub(crate) fn page_start(
    config: &BrowserSessionConfig,
    chrome: &DriverProcess,
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
) -> PageStart {
    if config.browser.isolated_page {
        if !chrome.is_attached() {
            // A browser that this session starts itself has a profile folder of
            // its own; its page is no page of a context, so this request cannot
            // be proven here.
            return PageStart::refused(
                "this session starts its own browser, whose page is a page of a profile folder and not of a browser "
                    .to_string()
                    + "context, so it cannot make one",
            );
        }
        return match isolated_page_start(config, chrome, link, stop_requested) {
            Ok(IsolatedStart::Resumed { target, context }) => PageStart {
                target: Some(target),
                resumed: true,
                context: Some(context),
                isolation: PageIsolation::IsolatedContext,
                refusal: None,
            },
            Ok(IsolatedStart::Fresh { context }) => PageStart {
                target: None,
                resumed: false,
                context: Some(context),
                isolation: PageIsolation::IsolatedContext,
                refusal: None,
            },
            Err(detail) => PageStart::refused(detail),
        };
    }
    let (resumed, target) = caller_page(config, chrome, link, stop_requested);
    PageStart {
        target,
        resumed,
        context: None,
        isolation: proven_isolation(config, chrome.is_attached()),
        refusal: None,
    }
}

/// What a page that asked for nothing proves about itself: a page of the folder
/// of a private panel of this computer keeps nothing, and every other page reads
/// the profile that it named.
fn proven_isolation(config: &BrowserSessionConfig, attached: bool) -> PageIsolation {
    if attached {
        // A page of the default context of a browser that another process owns
        // reads that browser's profile, whatever profile name this panel saved.
        return PageIsolation::Shared;
    }
    if config.browser.profile.as_deref() == Some(crate::PRIVATE_PROFILE) {
        return PageIsolation::PrivateFolder;
    }
    PageIsolation::Shared
}

/// Create an isolated browser context of the browser that this session drives,
/// for a private page: its cookies and its storage are apart from the profile,
/// and `Target.disposeBrowserContext` removes them with the context.
pub(crate) fn create_browser_context(link: &mut CdpLink, stop_requested: &AtomicBool) -> Result<String, String> {
    call_during_startup(
        link,
        stop_requested,
        "Target.createBrowserContext",
        &serde_json::json!({}),
    )
    .map_err(|error| format!("the browser refused a browser context of its own ({error})"))?
    .get("browserContextId")
    .and_then(serde_json::Value::as_str)
    .filter(|context| !context.is_empty())
    .map(str::to_string)
    .ok_or_else(|| "the browser answered no name for the browser context of this page".to_string())
}

/// Whether the browser still holds the isolated context `context`, so a private
/// page of the link before can come back inside it.
fn context_exists(link: &mut CdpLink, stop_requested: &AtomicBool, context: &str) -> bool {
    call_during_startup(
        link,
        stop_requested,
        "Target.getBrowserContexts",
        &serde_json::json!({}),
    )
    .ok()
    .and_then(|result| result.get("browserContextIds").and_then(|ids| ids.as_array()).cloned())
    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(context)))
}

/// The start of the private page of a panel: the page of the link before inside
/// its own context, or a new context for a first page or for one whose context
/// the browser no longer holds.
pub(crate) enum IsolatedStart {
    /// The page of the link before, which is still a page of its own context.
    Resumed { target: String, context: String },
    /// A new context, because this is the first page of the panel or because the
    /// context of the page before is gone.
    Fresh { context: String },
}

/// Start the private page of a panel of a browser that another process owns.
///
/// The page is a page of an isolated browser context in either case, and never a
/// page of the default context of that browser: the panel's page came back only
/// when its own context still holds it, and otherwise a new context carries a new
/// page. A browser that refuses a context gets the words of
/// [`crate::private_page_unavailable`], so the panel opens no page of the shared
/// profile in its place.
///
/// # Errors
/// The browser refused an isolated context, or answered no name for it.
pub(crate) fn isolated_page_start(
    config: &BrowserSessionConfig,
    chrome: &DriverProcess,
    link: &mut CdpLink,
    stop_requested: &AtomicBool,
) -> Result<IsolatedStart, String> {
    let from_link_before = config.resume_context.as_deref().zip(config.resume_target.as_deref());
    if let Some((context, target)) = from_link_before
        && context_exists(link, stop_requested, context)
        && let Some(target) = existing_page_target(link, stop_requested, target, Some(context))
    {
        if let DriverProcess::Attached(browser) = chrome {
            browser.set_context(context.to_string());
        }
        return Ok(IsolatedStart::Resumed {
            target,
            context: context.to_string(),
        });
    }
    let context = create_browser_context(link, stop_requested)?;
    if let DriverProcess::Attached(browser) = chrome {
        browser.set_context(context.clone());
    }
    Ok(IsolatedStart::Fresh { context })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::FrameSlot;
    use std::sync::Arc;

    fn config(profile: Option<&str>, isolated_page: bool) -> BrowserSessionConfig {
        BrowserSessionConfig {
            browser: crate::BrowserConfig {
                profile: profile.map(str::to_string),
                isolated_page,
                ..crate::BrowserConfig::default()
            },
            panel_local_id: "panel".to_string(),
            initial_url: None,
            width: 1280,
            height: 800,
            frame_slot: Arc::new(FrameSlot::new()),
            coordination: None,
            capture_directory: None,
            video: Arc::new(crate::VideoCaptureHandle::default()),
            remote: None,
            attach: None,
            resume_target: None,
            resume_context: None,
            page_isolation: Default::default(),
        }
    }

    /// A private page of this computer keeps its own folder, and every other page
    /// of a browser that this session starts reads the profile that it named.
    #[test]
    fn a_private_page_of_this_computer_is_its_own_folder() {
        assert_eq!(
            proven_isolation(&config(Some(crate::PRIVATE_PROFILE), false), false),
            PageIsolation::PrivateFolder
        );
        assert_eq!(
            proven_isolation(&config(Some(crate::DEFAULT_PROFILE), false), false),
            PageIsolation::Shared
        );
        assert_eq!(proven_isolation(&config(None, false), false), PageIsolation::Shared);
    }

    /// A page of a browser that another process owns proves nothing by the name of
    /// the profile that its panel saved: only an isolated context makes it private.
    #[test]
    fn a_page_of_a_borrowed_browser_proves_nothing_by_its_profile_name() {
        assert_eq!(
            proven_isolation(&config(Some(crate::PRIVATE_PROFILE), true), true),
            PageIsolation::Shared,
            "a page of the default context reads the profile of that browser"
        );
    }
}
