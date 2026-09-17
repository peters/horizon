use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::process::ChromeProcessControl;
use crate::session::{BrowserEvent, BrowserEventSender, BrowserSessionConfig};
use crate::websocket::JsonWsLink;
use crate::{AutomationDisclosurePolicy, BackendKind};

use super::super::host::DriverHost;
use super::super::remote::{
    AllocationRefusal, RemoteHost, RemoteReleaseOutcome, RemoteSessionEvent, RemoteStartFailure,
};
use super::super::service::WebDriverService;
use super::bidi::{connect_bidi_with_startup_retry, discover_context, install_common_signal_preload, subscribe};
use super::handshake::{NewSession, parse_new_session_response};
use super::semantic;

/// Spawn the local driver process and create its classic session.
pub(super) fn start_local(
    config: &BrowserSessionConfig,
    process_control: &ChromeProcessControl,
    stop_requested: &AtomicBool,
) -> Result<(DriverHost, NewSession), String> {
    let host = DriverHost::Local(WebDriverService::start(&config.browser, process_control, || {
        stop_requested.load(Ordering::Acquire)
    })?);
    let response = semantic::create_webdriver_session_response(host.transport(), config)?;
    let session = parse_new_session_response(&response)?;
    Ok((host, session))
}

/// Bind the remote transport and allocate exactly once, reporting the
/// lifecycle facts the panel shows. An ambiguous result is surfaced as
/// `AllocationUnknown` and never retried here.
pub(super) fn start_remote(
    request: &super::super::remote::RemoteSessionRequest,
    event_tx: &BrowserEventSender,
    remote_release: &crate::session::RemoteReleaseReport,
    stop_requested: &AtomicBool,
) -> Result<
    (
        DriverHost,
        NewSession,
        super::super::remote::identity::RemoteDeviceIdentity,
    ),
    String,
> {
    let label = request.label.clone();
    // A panel closed before the driver got here never asks the provider:
    // the report stays NeverAllocated and no slot is consumed.
    if stop_requested.load(Ordering::Acquire) {
        let _ = event_tx.send(BrowserEvent::RemoteSession(RemoteSessionEvent::AllocationFailed {
            label,
            reason: "the panel was closed before the remote session was requested".to_string(),
            refusal: AllocationRefusal::Other,
        }));
        return Err("remote session not requested: the panel was closed first".to_string());
    }
    let _ = event_tx.send(BrowserEvent::RemoteSession(RemoteSessionEvent::Allocating {
        label: label.clone(),
    }));
    // From here the provider may hold a device: nothing is established
    // until New Session answers or the failure below says otherwise.
    *remote_release.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    let allocated = RemoteHost::connect(request).and_then(|mut host| {
        let allocation = host.allocate(request)?;
        Ok((host, allocation))
    });
    match allocated {
        Ok((host, allocation)) => {
            let _ = event_tx.send(BrowserEvent::RemoteSession(RemoteSessionEvent::Allocated {
                label: label.clone(),
                session_digest: session_digest(&allocation.session.id),
            }));
            let _ = event_tx.send(BrowserEvent::RemoteSession(RemoteSessionEvent::DeviceIdentity {
                label,
                identity: allocation.device.clone(),
            }));
            Ok((DriverHost::Remote(host), allocation.session, allocation.device))
        }
        Err(failure) => {
            // Every failure ends the lifecycle with a terminal, value-free
            // event, so the panel never stays at "allocating"; what the
            // teardown reports decides whether the provider may still hold
            // a device.
            let (established, event) = start_failure_outcome(&failure, label);
            *remote_release.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = established;
            let _ = event_tx.send(BrowserEvent::RemoteSession(event));
            Err(failure.to_string())
        }
    }
}

/// What a failed remote start established at the provider, and the event
/// the panel sees. Only a refusal, or an immediate release the provider
/// confirmed, frees the slot; an unknown allocation, or an allocated session
/// whose immediate release was refused or unanswered, keeps holding it.
pub(super) fn start_failure_outcome(
    failure: &RemoteStartFailure,
    label: String,
) -> (Option<RemoteReleaseOutcome>, RemoteSessionEvent) {
    match failure {
        RemoteStartFailure::AllocationUnknown { reason } => (
            None,
            RemoteSessionEvent::AllocationUnknown {
                label,
                reason: reason.clone(),
            },
        ),
        RemoteStartFailure::AllocationFailed { error, message } => (
            Some(RemoteReleaseOutcome::NeverAllocated),
            RemoteSessionEvent::AllocationFailed {
                label,
                reason: failure.to_string(),
                refusal: AllocationRefusal::classify(error, message),
            },
        ),
        RemoteStartFailure::InvalidEndpoint(_) => (
            Some(RemoteReleaseOutcome::NeverAllocated),
            RemoteSessionEvent::AllocationFailed {
                label,
                reason: failure.to_string(),
                refusal: AllocationRefusal::Other,
            },
        ),
        RemoteStartFailure::IdentityRejected { reason, released } => (
            Some(released.clone()),
            RemoteSessionEvent::DeviceRejected {
                label,
                reason: reason.clone(),
                released: released.clone(),
            },
        ),
        RemoteStartFailure::Unenforceable { released } => {
            let event = match released {
                RemoteReleaseOutcome::Released
                | RemoteReleaseOutcome::AlreadyGone
                | RemoteReleaseOutcome::NeverAllocated => RemoteSessionEvent::AllocationFailed {
                    label,
                    reason: failure.to_string(),
                    refusal: AllocationRefusal::Other,
                },
                RemoteReleaseOutcome::ReleaseUnknown { .. } | RemoteReleaseOutcome::Failed { .. } => {
                    RemoteSessionEvent::AllocationUnknown {
                        label,
                        reason: failure.to_string(),
                    }
                }
            };
            (Some(released.clone()), event)
        }
    }
}

/// The optional `BiDi` channel a session starts with.
pub(super) struct BidiLink {
    pub(super) bidi: Option<JsonWsLink>,
    pub(super) context_id: Option<String>,
    pub(super) automation_ws: String,
}

/// Connect the `BiDi` channel a local browser advertised. A remote grid may
/// advertise one too; Horizon never connects to a returned endpoint with the
/// provider credential, so remote sessions stay on classic `WebDriver`
/// whatever the local backend is.
pub(super) fn establish_bidi(
    config: &BrowserSessionConfig,
    host: &mut DriverHost,
    session_id: &str,
    capabilities: &Value,
    stop_requested: &AtomicBool,
) -> Result<BidiLink, String> {
    let firefox_bidi = firefox_bidi_mode(config, host);
    let ws_url = if host.is_remote() {
        None
    } else {
        capabilities.get("webSocketUrl").and_then(Value::as_str)
    };
    let mut bidi = match ws_url {
        Some(url) => match connect_bidi_with_startup_retry(url, stop_requested) {
            Ok(link) => Some(link),
            Err(error) if config.browser.backend == BackendKind::SafariWebDriver => {
                tracing::warn!("Safari BiDi endpoint was unavailable; continuing with classic WebDriver: {error}");
                None
            }
            Err(error) => return Err(error),
        },
        None => None,
    };
    if firefox_bidi && bidi.is_none() {
        host.delete_session(session_id);
        return Err("Firefox did not return the required WebDriver BiDi webSocketUrl".to_string());
    }
    let shared = host.shared_context().is_some();
    let mut context_id = host
        .shared_context()
        .map(str::to_owned)
        .or_else(|| bidi.as_mut().and_then(discover_context));
    if firefox_bidi && context_id.is_none() {
        host.delete_session(session_id);
        return Err("Firefox BiDi returned no top-level browsing context".to_string());
    }
    if shared && let Some(link) = bidi.as_mut() {
        super::bidi::subscribe_shared_page(link, host, config.browser.automation_disclosure)?;
    }
    if !shared
        && firefox_bidi
        && config.browser.automation_disclosure == AutomationDisclosurePolicy::MinimizeCommonSignals
        && let Some(link) = bidi.as_mut()
        && let Err(error) = install_common_signal_preload(link)
    {
        host.delete_session(session_id);
        return Err(format!(
            "Firefox could not install pre-document automation disclosure minimization: {error}"
        ));
    }
    if !shared
        && let Some(link) = bidi.as_mut()
        && let Err(error) = subscribe(link, config.browser.backend, context_id.as_deref())
    {
        if firefox_bidi {
            host.delete_session(session_id);
            return Err(format!("Firefox BiDi event subscription failed: {error}"));
        }
        bidi = None;
        context_id = None;
    }
    let automation_ws = bidi.as_ref().and(ws_url).unwrap_or_default().to_string();
    Ok(BidiLink {
        bidi,
        context_id,
        automation_ws,
    })
}

/// Firefox `BiDi` is a local-host mode only; see [`super::Driver::firefox_bidi`].
pub(super) fn firefox_bidi_mode(config: &BrowserSessionConfig, host: &DriverHost) -> bool {
    config.browser.backend == BackendKind::FirefoxBidi && !host.is_remote()
}

/// Short, non-reversible correlation id for a provider session id.
fn session_digest(session_id: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(session_id, &mut hasher);
    format!("{:016x}", std::hash::Hasher::finish(&hasher))
}
