//! Classic `WebDriver` guards use provider-issued root references and native URLs.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{BackendKind, BrowserControlFailure};

use super::{Driver, webdriver_value};

pub(super) const OBSERVATION_BUDGET: Duration = crate::wait::RELEASE_CHECK_BUDGET;

#[derive(Debug, PartialEq, Eq)]
struct Anchor {
    url: String,
    root: String,
}

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
        let first = self.native_document_anchor(deadline)?;
        let second = self.native_document_anchor(deadline)?;
        if first != second {
            self.invalidate_classic_document();
            return Err(BrowserControlFailure::new(
                "document_navigation_invalidated",
                "the page changed during native document observation",
            ));
        }
        let identity = serde_json::to_string(&[second.url, second.root]).map_err(|_| invalid_anchor())?;
        Ok(self.record_classic_document_identity(&identity))
    }

    fn native_document_anchor(&self, deadline: Instant) -> Result<Anchor, BrowserControlFailure> {
        let result = self.classic_get_within("url", remaining(deadline)?);
        remaining(deadline)?;
        let url_response = result.map_err(|_| invalid_anchor())?;
        let url = webdriver_value(&url_response)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid_anchor)?
            .to_owned();
        let result = self.classic_navigation_post_within(
            "element",
            &json!({"using":"css selector","value":":root"}),
            remaining(deadline)?,
        );
        remaining(deadline)?;
        let root_response = result.map_err(|_| invalid_anchor())?;
        let root = webdriver_value(&root_response)
            .and_then(|value| {
                value
                    .get("element-6066-11e4-a52e-4f735466cecf")
                    .or_else(|| value.get("ELEMENT"))
            })
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid_anchor)?
            .to_owned();
        Ok(Anchor { url, root })
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

    fn record_classic_document_identity(&mut self, identity: &str) -> bool {
        let changed = self
            .classic_document_identity
            .replace(identity.to_owned())
            .is_some_and(|previous| previous != identity);
        if changed {
            self.invalidate_classic_document();
        }
        changed
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
