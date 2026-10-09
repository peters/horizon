//! Classic `WebDriver` session creation: capabilities per backend, the
//! New Session response, and the Safari window handle read once at startup.

use serde_json::{Map, Value, json};

use crate::process::resolve_binary;
use crate::session::BrowserSessionConfig;
use crate::{BackendKind, BrowserConfig};

use super::super::http::HttpError;
use super::super::service::prepare_profile;
use super::super::transport::ClassicTransport;
use super::{PAGE_LOAD_TIMEOUT_MILLIS, safari};

#[derive(Debug)]
pub(in crate::webdriver) struct NewSession {
    pub(in crate::webdriver) id: String,
    pub(in crate::webdriver) capabilities: Value,
}

pub(super) fn initial_safari_input(
    transport: &dyn ClassicTransport,
    session_id: &str,
    backend: BackendKind,
) -> Result<Option<safari::InputState>, String> {
    if backend != BackendKind::SafariWebDriver {
        return Ok(None);
    }
    let response = transport
        .get(&format!("/session/{session_id}/window"))
        .map_err(|error| format!("failed to read Safari window handle: {error}"))?;
    safari::InputState::from_window_response(&response).map(Some)
}

pub(super) fn create_webdriver_session(
    transport: &dyn ClassicTransport,
    config: &BrowserSessionConfig,
    request_bidi: bool,
) -> Result<Value, HttpError> {
    let capabilities = new_session_capabilities(
        &config.browser,
        &config.panel_local_id,
        request_bidi,
        config.frame_slot.file_chooser().has_consumer(),
    )
    .map_err(|error| HttpError::InvalidResponse(format!("invalid session capabilities: {error}")))?;
    transport.post("/session", &json!({ "capabilities": { "alwaysMatch": capabilities } }))
}

pub(super) fn new_session_capabilities(
    config: &BrowserConfig,
    panel_local_id: &str,
    request_bidi: bool,
    manual_file_chooser: bool,
) -> Result<Value, String> {
    match config.backend {
        BackendKind::FirefoxBidi => {
            validate_firefox_args(&config.extra_args)?;
            let profile = config.profile_dir(panel_local_id);
            prepare_profile(&profile)?;
            let mut options = Map::new();
            let mut args = Vec::with_capacity(5 + config.extra_args.len());
            if config.headless {
                args.push("-headless".to_string());
            }
            args.extend([
                "-no-remote".to_string(),
                "-profile".to_string(),
                profile.to_string_lossy().to_string(),
            ]);
            args.extend(config.extra_args.iter().cloned());
            options.insert("args".to_string(), json!(args));
            // Headless Firefox otherwise inherits GTK overlay scrollbars,
            // which fade completely out of screenshots and leave a streamed
            // browser panel with no visible drag target.
            options.insert(
                "prefs".to_string(),
                json!({
                    "widget.gtk.overlay-scrollbars.enabled": false,
                    "ui.useOverlayScrollbars": 0,
                    "remote.bidi.dismiss_file_pickers.enabled": manual_file_chooser,
                }),
            );
            if let Some(command) = &config.firefox_command {
                let binary = resolve_binary(command).map_err(|error| error.to_string())?;
                options.insert("binary".to_string(), json!(binary));
            }
            Ok(json!({
                "browserName": "firefox",
                "webSocketUrl": true,
                "acceptInsecureCerts": false,
                "pageLoadStrategy": "eager",
                "timeouts": { "pageLoad": PAGE_LOAD_TIMEOUT_MILLIS },
                "moz:firefoxOptions": Value::Object(options),
            }))
        }
        BackendKind::SafariWebDriver => {
            let mut capabilities = Map::new();
            capabilities.insert("browserName".to_string(), json!("safari"));
            capabilities.insert("acceptInsecureCerts".to_string(), json!(false));
            capabilities.insert("timeouts".to_string(), json!({ "pageLoad": PAGE_LOAD_TIMEOUT_MILLIS }));
            if request_bidi {
                capabilities.insert("webSocketUrl".to_string(), json!(true));
            }
            Ok(Value::Object(capabilities))
        }
        BackendKind::ChromiumCdp => Err("Chromium does not create a WebDriver session".to_string()),
    }
}

/// Outcome of clearing Firefox's native `navigator.webdriver` flag.
pub(in crate::webdriver) enum FirefoxNativeFlagClear {
    /// Content processes will see a native getter that returns false.
    Cleared,
    /// The session is still in the content context, so the preload can run.
    Unavailable(String),
    /// The session was left in the chrome context and must be discarded.
    Unsafe(String),
}

/// Clear the Marionette and Remote Agent automation flags before any
/// caller-supplied document is created. Restores the content context first.
pub(in crate::webdriver) fn clear_firefox_native_automation_flag(
    transport: &dyn ClassicTransport,
    session_id: &str,
) -> FirefoxNativeFlagClear {
    let session = format!("/session/{session_id}");
    // A lost response can still leave Firefox in the chrome context. Always
    // try to return to content, and discard the session if that is unconfirmed.
    let chrome_switch = transport.post(&format!("{session}/moz/context"), &json!({ "context": "chrome" }));
    let executed = if chrome_switch.is_ok() {
        Some(transport.post(
            &format!("{session}/execute/sync"),
            &json!({
                "script": crate::disclosure::FIREFOX_NATIVE_AUTOMATION_FLAG_SCRIPT,
                "args": []
            }),
        ))
    } else {
        None
    };
    if let Err(error) = transport.post(&format!("{session}/moz/context"), &json!({ "context": "content" })) {
        return FirefoxNativeFlagClear::Unsafe(format!("Firefox could not return from the chrome context: {error}"));
    }
    match chrome_switch {
        Err(error) => FirefoxNativeFlagClear::Unavailable(format!("Firefox chrome context was unavailable: {error}")),
        Ok(_) => match executed {
            Some(Ok(response)) if crate::disclosure::firefox_native_automation_flag_cleared(&response) => {
                FirefoxNativeFlagClear::Cleared
            }
            Some(Ok(_)) => {
                FirefoxNativeFlagClear::Unavailable("Firefox left the native automation flag set".to_string())
            }
            Some(Err(error)) => FirefoxNativeFlagClear::Unavailable(format!(
                "Firefox could not clear the native automation flag: {error}"
            )),
            None => FirefoxNativeFlagClear::Unavailable("Firefox chrome context was not confirmed".to_string()),
        },
    }
}

pub(super) fn validate_firefox_args(arguments: &[String]) -> Result<(), String> {
    for argument in arguments {
        let normalized = argument.trim_start_matches('-').to_ascii_lowercase();
        if matches!(normalized.as_str(), "profile" | "p" | "marionette") || normalized.starts_with("remote-debugging-")
        {
            return Err(format!(
                "browser.extra_args cannot override managed Firefox argument {argument:?}"
            ));
        }
    }
    Ok(())
}

pub(super) fn safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(in crate::webdriver) fn parse_new_session_response(response: &Value) -> Result<NewSession, String> {
    let value = response.get("value").unwrap_or(response);
    let id = value
        .get("sessionId")
        .or_else(|| response.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|id| safe_session_id(id))
        .ok_or_else(|| "WebDriver returned no safe session id".to_string())?
        .to_string();
    let capabilities = value
        .get("capabilities")
        .or_else(|| response.get("capabilities"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(NewSession { id, capabilities })
}
