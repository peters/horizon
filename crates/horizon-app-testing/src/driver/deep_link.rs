//! Accept only the system confirmation for an iOS deep link, never permission alerts.
use super::{COMMAND_TIMEOUT, NativeDriver, json};
use crate::{Error, Result};
use std::time::{Duration, Instant};

pub(super) fn open(driver: &NativeDriver, url: &str) -> Result<()> {
    let outcome = driver.execute("mobile: deepLink", &json!({"bundleId": driver.app_id, "url": url}));
    // Never replay an uncertain effectful request, and preserve the unsupported driver reason.
    if outcome.is_err() {
        return outcome.map(|_| ());
    }
    let started = Instant::now();
    let poll_deadline = started + Duration::from_secs(2);
    let command_deadline = started + COMMAND_TIMEOUT;
    let session_deadline = driver.deadline.into_iter().chain(driver.lifetime_deadline).min();
    loop {
        if session_deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err(Error::WaitTimeout);
        }
        if Instant::now() >= poll_deadline {
            return Ok(());
        }
        // A last negative probe still needs its normal response budget.
        let remaining = command_deadline.saturating_duration_since(Instant::now());
        match driver.request_with_limit("GET", "/alert/text", None, remaining) {
            Err(Error::AlertMissing) if Instant::now() < poll_deadline => {
                let until = session_deadline.map_or(poll_deadline, |limit| limit.min(poll_deadline));
                std::thread::sleep(
                    until
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(100)),
                );
            }
            Err(Error::AlertMissing) => return Ok(()),
            Err(error) => return Err(error),
            Ok(response) => {
                let text = response
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(Error::DriverInvalid)?;
                if !text.starts_with("Open in ") && !text.starts_with("Open this page in ") {
                    return Err(Error::DeepLinkConfirmationBlocked);
                }
                let buttons = alert(driver, command_deadline, &json!({"action": "getButtons"}))?;
                let buttons = buttons
                    .get("value")
                    .and_then(serde_json::Value::as_array)
                    .ok_or(Error::DriverInvalid)?;
                if buttons.len() != 2
                    || !buttons.iter().any(|button| button.as_str() == Some("Open"))
                    || !buttons.iter().any(|button| button.as_str() == Some("Cancel"))
                {
                    return Err(Error::DeepLinkConfirmationBlocked);
                }
                return alert(
                    driver,
                    command_deadline,
                    &json!({"action": "accept", "buttonLabel": "Open"}),
                )
                .map(|_| ());
            }
        }
    }
}

fn alert(driver: &NativeDriver, deadline: Instant, arguments: &serde_json::Value) -> Result<serde_json::Value> {
    driver.request_with_limit(
        "POST",
        "/execute/sync",
        Some(&json!({"script": "mobile: alert", "args": [arguments]})),
        deadline.saturating_duration_since(Instant::now()),
    )
}

#[cfg(test)]
mod tests;
