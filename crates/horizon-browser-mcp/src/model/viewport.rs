//! Public CSS viewport resize and release contract.

use horizon_browser::{BrowserControlAction, BrowserControlValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ResizeInput {
    pub(crate) panel_id: String,
    /// Content viewport width in CSS pixels (320-8000). Required with height unless reset=true.
    pub(crate) width: Option<u32>,
    /// Content viewport height in CSS pixels (320-8000). Required with width unless reset=true.
    pub(crate) height: Option<u32>,
    /// Resume host panel sizing. Omit width and height when true.
    #[serde(default)]
    pub(crate) reset: bool,
    /// Engine measurement deadline (1-60000 ms, default 15000); a timeout may follow an applied mutation.
    pub(crate) timeout_millis: Option<u64>,
}

impl ResizeInput {
    pub(crate) fn timeout_millis(&self) -> u64 {
        self.timeout_millis.unwrap_or(15_000)
    }

    pub(crate) fn action(&self) -> Result<BrowserControlAction, String> {
        let viewport = match (self.reset, self.width, self.height) {
            (true, None, None) => None,
            (false, Some(width), Some(height)) => Some([width, height]),
            _ => return Err("invalid_input: supply both width and height, or reset=true without dimensions".into()),
        };
        let action = BrowserControlAction::Resize {
            viewport,
            timeout_millis: self.timeout_millis(),
        };
        action
            .validate()
            .map_err(|message| format!("invalid_input: {message}"))?;
        Ok(action)
    }
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
pub(crate) struct ViewportSize {
    width: u32,
    height: u32,
}

impl From<[u32; 2]> for ViewportSize {
    fn from([width, height]: [u32; 2]) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ResizeOutput {
    panel_id: String,
    action_id: String,
    /// Null when released to panel sizing.
    requested: Option<ViewportSize>,
    /// Actual page innerWidth and innerHeight measured by the browser.
    applied: ViewportSize,
}

impl ResizeOutput {
    pub(crate) fn from_result(
        panel_id: String,
        action_id: String,
        value: &BrowserControlValue,
    ) -> Result<Self, String> {
        let BrowserControlValue::Viewport { requested, applied } = value else {
            return Err("browser returned an unexpected viewport result".into());
        };
        if requested.is_some_and(|size| size != *applied) || applied.contains(&0) {
            return Err("browser returned an inconsistent viewport result".into());
        }
        Ok(Self {
            panel_id,
            action_id,
            requested: requested.map(Into::into),
            applied: (*applied).into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_and_reset_inputs_validate_before_claiming_a_panel() {
        for value in [
            json!({"panel_id":"p", "width":390,"height":844}),
            json!({"panel_id":"p", "reset":true}),
        ] {
            assert!(serde_json::from_value::<ResizeInput>(value).unwrap().action().is_ok());
        }
        for value in [
            json!({"panel_id":"p"}),
            json!({"panel_id":"p","width":390}),
            json!({"panel_id":"p","reset":true,"width":390,"height":844}),
            json!({"panel_id":"p","width":0,"height":844}),
            json!({"panel_id":"p","width":390,"height":8001}),
            json!({"panel_id":"p","reset":true,"timeout_millis":0}),
            json!({"panel_id":"p","reset":true,"timeout_millis":60001}),
        ] {
            assert!(serde_json::from_value::<ResizeInput>(value).unwrap().action().is_err());
        }
    }

    #[test]
    fn output_requires_measurement_and_separates_reset_from_requested_dimensions() {
        assert!(ResizeOutput::from_result("p".into(), "a".into(), &BrowserControlValue::Accepted).is_err());
        assert!(
            ResizeOutput::from_result(
                "p".into(),
                "a".into(),
                &BrowserControlValue::Viewport {
                    requested: Some([390, 844]),
                    applied: [900, 600]
                }
            )
            .is_err()
        );
        let output = ResizeOutput::from_result(
            "p".into(),
            "a".into(),
            &BrowserControlValue::Viewport {
                requested: None,
                applied: [900, 600],
            },
        )
        .unwrap();
        let value = serde_json::to_value(output).unwrap();
        assert!(value["requested"].is_null());
        assert_eq!(value["applied"], json!({"width":900,"height":600}));
    }
}
