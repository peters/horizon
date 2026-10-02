//! Classic `WebDriver` guards use provider-issued root references and native URLs.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{BackendKind, BrowserControlFailure};

use super::super::{http::HttpError, transport::encode_path_segment};
use super::{Driver, webdriver_value};

pub(super) const OBSERVATION_BUDGET: Duration = crate::wait::RELEASE_CHECK_BUDGET;

impl Driver {
    fn tracks_classic_document_identity(&self) -> bool {
        self.config.browser.backend == BackendKind::SafariWebDriver || self.host.is_remote()
    }

    pub(super) fn initialize_classic_document_identity(&mut self) {
        if self.tracks_classic_document_identity() {
            let _ = self.refresh_classic_document_identity_within(OBSERVATION_BUDGET);
        }
    }

    pub(super) fn refresh_classic_document_identity_within(
        &mut self,
        timeout: Duration,
    ) -> Result<bool, BrowserControlFailure> {
        if !self.tracks_classic_document_identity() {
            return Ok(false);
        }
        let deadline = Instant::now() + timeout;
        let url = self.native_document_url(deadline)?;
        let previous = self
            .classic_document_identity
            .as_deref()
            .and_then(|value| serde_json::from_str::<[String; 2]>(value).ok());
        let mut stale = previous.as_ref().is_some_and(|value| value[0] != url);
        if stale {
            self.invalidate_classic_document();
        }
        let root = if let Some([_, root]) = previous.filter(|value| value[0] == url) {
            if self.native_root_is_current(&root, deadline)? {
                root
            } else {
                // A provider may reuse the opaque string for a replacement.
                // Native staleness invalidates independently of ID equality.
                self.invalidate_classic_document();
                stale = true;
                self.find_native_root(deadline)?
            }
        } else {
            self.find_native_root(deadline)?
        };
        if self.native_document_url(deadline)? != url {
            self.invalidate_classic_document();
            return Err(BrowserControlFailure::new(
                "document_navigation_invalidated",
                "the page changed during native document observation",
            ));
        }
        let identity = serde_json::to_string(&[url, root]).map_err(|_| invalid_anchor())?;
        let changed = self
            .classic_document_identity
            .replace(identity.clone())
            .is_some_and(|previous| previous != identity);
        if changed && !stale {
            self.invalidate_classic_document();
        }
        Ok(stale || changed)
    }

    fn native_document_url(&self, deadline: Instant) -> Result<String, BrowserControlFailure> {
        let result = self.classic_get_within("url", remaining(deadline)?);
        remaining(deadline)?;
        let response = result.map_err(|_| invalid_anchor())?;
        webdriver_value(&response)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(invalid_anchor)
    }

    fn find_native_root(&mut self, deadline: Instant) -> Result<String, BrowserControlFailure> {
        let result = self.classic_navigation_post_within(
            "element",
            &json!({"using":"css selector","value":":root"}),
            remaining(deadline)?,
        );
        remaining(deadline)?;
        let response = result.map_err(|_| invalid_anchor())?;
        let root = webdriver_value(&response)
            .and_then(|value| {
                value
                    .get("element-6066-11e4-a52e-4f735466cecf")
                    .or_else(|| value.get("ELEMENT"))
            })
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid_anchor)?
            .to_owned();
        if !self.native_root_is_current(&root, deadline)? {
            self.invalidate_classic_document();
            return Err(BrowserControlFailure::new(
                "document_navigation_invalidated",
                "the replacement root became stale during native observation",
            ));
        }
        Ok(root)
    }

    fn native_root_is_current(&self, root: &str, deadline: Instant) -> Result<bool, BrowserControlFailure> {
        let path = self.session_path(&format!("element/{}/name", encode_path_segment(root)));
        let result = self.host.transport().get_with_read_timeout(&path, remaining(deadline)?);
        remaining(deadline)?;
        match result {
            Err(HttpError::WebDriver { error, .. })
                if matches!(error.as_str(), "stale element reference" | "no such element") =>
            {
                Ok(false)
            }
            Ok(response)
                if webdriver_value(&response)
                    .and_then(Value::as_str)
                    .is_some_and(|name| !name.is_empty()) =>
            {
                Ok(true)
            }
            _ => Err(invalid_anchor()),
        }
    }

    pub(super) fn guarded_semantic_scan(
        &mut self,
        expression: &str,
        timeout: Option<Duration>,
    ) -> Result<Value, BrowserControlFailure> {
        if !self.tracks_classic_document_identity() {
            return self.evaluate_json_within(expression, timeout);
        }
        let deadline = Instant::now() + timeout.unwrap_or(super::super::transport::DEFAULT_READ_TIMEOUT);
        self.refresh_classic_document_identity_within(remaining(deadline)?)?;
        let result = self.evaluate_json_within(expression, Some(remaining(deadline)?));
        remaining(deadline)?;
        let value = result?;
        if self.refresh_classic_document_identity_within(remaining(deadline)?)? {
            return Err(BrowserControlFailure::new(
                "document_navigation_invalidated",
                "the page changed during semantic observation; take a fresh snapshot",
            ));
        }
        Ok(value)
    }

    fn invalidate_classic_document(&mut self) {
        self.invalidate_document_orientation();
        self.semantic.invalidate();
        self.advance_generation();
    }
}

fn remaining(deadline: Instant) -> Result<Duration, BrowserControlFailure> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| {
            BrowserControlFailure::new(
                "document_observation_timeout",
                "the native document observation exceeded its original bound",
            )
        })
}

fn invalid_anchor() -> BrowserControlFailure {
    BrowserControlFailure::new(
        "invalid_result",
        "WebDriver returned no verifiable native document anchor",
    )
}

#[cfg(test)]
mod tests;
