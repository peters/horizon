//! Remote device orientation tool contract.
use horizon_browser::remote::RemoteOrientation;
use horizon_browser::{BrowserControlAction, BrowserControlValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Orientation {
    Portrait,
    Landscape,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Support {
    Supported,
    Unsupported,
    Unverified,
}

impl From<Orientation> for RemoteOrientation {
    fn from(value: Orientation) -> Self {
        match value {
            Orientation::Portrait => Self::Portrait,
            Orientation::Landscape => Self::Landscape,
        }
    }
}
impl From<RemoteOrientation> for Orientation {
    fn from(value: RemoteOrientation) -> Self {
        match value {
            RemoteOrientation::Portrait => Self::Portrait,
            RemoteOrientation::Landscape => Self::Landscape,
        }
    }
}
impl From<horizon_browser::remote::OrientationSupport> for Support {
    fn from(value: horizon_browser::remote::OrientationSupport) -> Self {
        match value {
            horizon_browser::remote::OrientationSupport::Supported => Self::Supported,
            horizon_browser::remote::OrientationSupport::Unsupported => Self::Unsupported,
            horizon_browser::remote::OrientationSupport::Unverified => Self::Unverified,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct OrientationInput {
    pub(crate) panel_id: String,
    pub(crate) orientation: Orientation,
    /// Measurement deadline, 1-60000 ms; default 15000. A timeout may follow mutation.
    pub(crate) timeout_millis: Option<u64>,
}
impl OrientationInput {
    pub(crate) fn timeout_millis(&self) -> u64 {
        self.timeout_millis.unwrap_or(RemoteOrientation::DEFAULT_TIMEOUT_MILLIS)
    }
    pub(crate) fn action(&self) -> Result<BrowserControlAction, String> {
        let action = BrowserControlAction::Orientation {
            orientation: self.orientation.into(),
            timeout_millis: self.timeout_millis(),
        };
        action
            .validate()
            .map_err(|message| format!("invalid_input: {message}"))?;
        Ok(action)
    }
}
#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct OrientationOutput {
    panel_id: String,
    action_id: String,
    requested: Orientation,
    applied: Orientation,
    /// Browser-measured innerWidth/innerHeight in CSS pixels.
    viewport: [u32; 2],
}
impl OrientationOutput {
    pub(crate) fn from_result(
        panel_id: String,
        action_id: String,
        value: &BrowserControlValue,
    ) -> Result<Self, String> {
        let BrowserControlValue::Orientation {
            requested,
            applied,
            viewport,
        } = value
        else {
            return Err("browser returned an unexpected orientation result".into());
        };
        let axes_match = match applied {
            RemoteOrientation::Portrait => viewport[0] < viewport[1],
            RemoteOrientation::Landscape => viewport[0] > viewport[1],
        };
        if requested != applied || viewport.contains(&0) || !axes_match {
            return Err("browser returned an inconsistent orientation result".into());
        }
        Ok(Self {
            panel_id,
            action_id,
            requested: (*requested).into(),
            applied: (*applied).into(),
            viewport: *viewport,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn results_require_measured_axes_matching_the_requested_orientation() {
        for requested in [RemoteOrientation::Portrait, RemoteOrientation::Landscape] {
            for applied in [RemoteOrientation::Portrait, RemoteOrientation::Landscape] {
                for viewport in [[0, 0], [0, 800], [800, 0], [800, 800], [600, 800], [800, 600]] {
                    let value = BrowserControlValue::Orientation {
                        requested,
                        applied,
                        viewport,
                    };
                    let output = OrientationOutput::from_result("p".into(), "a".into(), &value);
                    let valid = requested == applied
                        && match requested {
                            RemoteOrientation::Portrait => viewport == [600, 800],
                            RemoteOrientation::Landscape => viewport == [800, 600],
                        };
                    assert_eq!(output.is_ok(), valid, "{value:?}");
                    if let Ok(output) = output {
                        assert_eq!(output.viewport, viewport);
                    }
                }
            }
        }
    }
    #[test]
    fn invalid_requests_and_unmeasured_results_are_refused() {
        for orientation in ["portrait", "landscape"] {
            let input: OrientationInput =
                serde_json::from_value(json!({"panel_id":"p","orientation":orientation})).unwrap();
            assert!(input.action().is_ok());
        }
        assert!(serde_json::from_value::<OrientationInput>(json!({"panel_id":"p","orientation":"sideways"})).is_err());
        let input: OrientationInput =
            serde_json::from_value(json!({"panel_id":"p","orientation":"portrait","timeout_millis":0})).unwrap();
        assert!(input.action().is_err());
        assert!(OrientationOutput::from_result("p".into(), "a".into(), &BrowserControlValue::Accepted).is_err());
    }
}
