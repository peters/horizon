//! Public CSS viewport resize, page zoom and release contract.

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
    /// Actual page innerWidth and innerHeight measured by the browser. Under a
    /// page zoom this is the requested size divided by `zoom_percent / 100`.
    applied: ViewportSize,
    /// The page zoom in effect (100 when the page is not zoomed).
    zoom_percent: u32,
}

impl ResizeOutput {
    pub(crate) fn from_result(
        panel_id: String,
        action_id: String,
        value: &BrowserControlValue,
    ) -> Result<Self, String> {
        let BrowserControlValue::Viewport {
            requested,
            applied,
            zoom_percent,
        } = value
        else {
            return Err("browser returned an unexpected viewport result".into());
        };
        if requested.is_some_and(|size| horizon_browser::zoomed_viewport(size, *zoom_percent) != *applied)
            || applied.contains(&0)
            || !valid_zoom(*zoom_percent)
        {
            return Err("browser returned an inconsistent viewport result".into());
        }
        Ok(Self {
            panel_id,
            action_id,
            requested: requested.map(Into::into),
            applied: (*applied).into(),
            zoom_percent: *zoom_percent,
        })
    }
}

fn valid_zoom(percent: u32) -> bool {
    (horizon_browser::MIN_ZOOM_PERCENT..=horizon_browser::MAX_ZOOM_PERCENT).contains(&percent)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ZoomInput {
    pub(crate) panel_id: String,
    /// Page zoom in percent (25-500), like the browser's own zoom level. Required unless reset=true.
    pub(crate) percent: Option<u32>,
    /// Return to 100 percent. Omit percent when true.
    #[serde(default)]
    pub(crate) reset: bool,
    /// Engine measurement deadline (1-60000 ms, default 15000); a timeout may follow an applied mutation.
    pub(crate) timeout_millis: Option<u64>,
}

impl ZoomInput {
    pub(crate) fn timeout_millis(&self) -> u64 {
        self.timeout_millis.unwrap_or(15_000)
    }

    pub(crate) fn action(&self) -> Result<BrowserControlAction, String> {
        let percent = match (self.reset, self.percent) {
            (true, None) => None,
            (false, Some(percent)) => Some(percent),
            _ => return Err("invalid_input: supply percent, or reset=true without percent".into()),
        };
        let action = BrowserControlAction::Zoom {
            percent,
            timeout_millis: self.timeout_millis(),
        };
        action
            .validate()
            .map_err(|message| format!("invalid_input: {message}"))?;
        Ok(action)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub(crate) struct ZoomOutput {
    panel_id: String,
    action_id: String,
    /// Null after a reset.
    requested_percent: Option<u32>,
    /// The zoom now in effect.
    applied_percent: u32,
    /// Page innerWidth and innerHeight measured by the browser at that zoom:
    /// the panel or `browser_resize` size divided by `applied_percent / 100`.
    css_viewport: ViewportSize,
}

impl ZoomOutput {
    pub(crate) fn from_result(
        panel_id: String,
        action_id: String,
        value: &BrowserControlValue,
    ) -> Result<Self, String> {
        let BrowserControlValue::Zoom {
            requested,
            applied,
            viewport,
        } = value
        else {
            return Err("browser returned an unexpected zoom result".into());
        };
        if requested.unwrap_or(100) != *applied || !valid_zoom(*applied) || viewport.contains(&0) {
            return Err("browser returned an inconsistent zoom result".into());
        }
        Ok(Self {
            panel_id,
            action_id,
            requested_percent: *requested,
            applied_percent: *applied,
            css_viewport: (*viewport).into(),
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
    fn zoom_inputs_validate_and_outputs_report_the_measured_viewport() {
        for value in [
            json!({"panel_id":"p", "percent":200}),
            json!({"panel_id":"p", "percent":25}),
            json!({"panel_id":"p", "reset":true}),
        ] {
            assert!(serde_json::from_value::<ZoomInput>(value).unwrap().action().is_ok());
        }
        for value in [
            json!({"panel_id":"p"}),
            json!({"panel_id":"p","percent":24}),
            json!({"panel_id":"p","percent":501}),
            json!({"panel_id":"p","reset":true,"percent":200}),
            json!({"panel_id":"p","percent":200,"timeout_millis":0}),
        ] {
            assert!(serde_json::from_value::<ZoomInput>(value).unwrap().action().is_err());
        }
        let output = ZoomOutput::from_result(
            "p".into(),
            "a".into(),
            &BrowserControlValue::Zoom {
                requested: Some(200),
                applied: 200,
                viewport: [640, 400],
            },
        )
        .unwrap();
        let value = serde_json::to_value(output).unwrap();
        assert_eq!(value["applied_percent"], 200);
        assert_eq!(value["css_viewport"], json!({"width":640,"height":400}));
        assert!(
            ZoomOutput::from_result(
                "p".into(),
                "a".into(),
                &BrowserControlValue::Zoom {
                    requested: Some(200),
                    applied: 150,
                    viewport: [640, 400],
                },
            )
            .is_err()
        );
        let zoomed_resize = ResizeOutput::from_result(
            "p".into(),
            "a".into(),
            &BrowserControlValue::Viewport {
                requested: Some([1280, 800]),
                applied: [320, 200],
                zoom_percent: 400,
            },
        )
        .unwrap();
        assert_eq!(serde_json::to_value(zoomed_resize).unwrap()["zoom_percent"], 400);
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
                    applied: [900, 600],
                    zoom_percent: 100,
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
                zoom_percent: 100,
            },
        )
        .unwrap();
        let value = serde_json::to_value(output).unwrap();
        assert!(value["requested"].is_null());
        assert_eq!(value["applied"], json!({"width":900,"height":600}));
    }
}
