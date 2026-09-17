//! The validated remote Android Chrome pointer path uses the visual viewport's CSS origin.
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Driver, webdriver_value};
use crate::BrowserControlFailure;
use crate::semantic::check_script_error;
use crate::webdriver::transport::ClassicTransport;

pub(super) fn uses_visual_viewport(remote: bool, capabilities: &Value) -> bool {
    remote
        && capabilities["platformName"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("android"))
        && capabilities["browserName"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("chrome"))
}

pub(super) fn use_touch_pointer(payload: &mut Value) {
    // Mouse press can dismiss the keyboard before release and move the tap.
    // A separate touch source keeps native mobile activation in one gesture.
    if let Some(sources) = payload["actions"].as_array_mut() {
        for source in sources.iter_mut().filter(|source| source["type"] == "pointer") {
            source["id"] = json!("horizon-touch");
            source["parameters"]["pointerType"] = json!("touch");
        }
    }
}

pub(super) fn click_through(transport: &dyn ClassicTransport, session: &str, payload: &Value) -> Result<(), String> {
    transport
        .post_with_read_timeout(&format!("{session}/actions"), payload, super::NAVIGATION_HTTP_TIMEOUT)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
struct ClickPoint {
    x: f64,
    y: f64,
    offset_x: f64,
    offset_y: f64,
    width: f64,
    height: f64,
}

impl ClickPoint {
    fn visual_coordinates(&self) -> Result<(f64, f64), BrowserControlFailure> {
        let x = self.x - self.offset_x;
        let y = self.y - self.offset_y;
        if ![x, y, self.width, self.height].iter().all(|n| n.is_finite())
            || x < 0.0
            || y < 0.0
            || x >= self.width
            || y >= self.height
        {
            return Err(BrowserControlFailure::new(
                "element_not_visible",
                "Target has no trustworthy visual viewport point",
            ));
        }
        Ok((x, y))
    }
}

impl Driver {
    pub(super) fn remote_click_point(&mut self, selector: &str) -> Result<(f64, f64), BrowserControlFailure> {
        let response = self
            .classic_navigation_post_within(
                "execute/async",
                &json!({"script": include_str!("remote_click.js"), "args": [selector]}),
                Duration::from_secs(4),
            )
            .map_err(|error| BrowserControlFailure::new("input_failed", error))?;
        let value = webdriver_value(&response)
            .and_then(|value| value.get("geometry"))
            .ok_or_else(|| BrowserControlFailure::new("invalid_result", "No click geometry returned"))?;
        check_script_error(value)?;
        let point: ClickPoint = serde_json::from_value(value.clone())
            .map_err(|_| BrowserControlFailure::new("invalid_result", "Invalid click geometry returned"))?;
        self.capture_teach_fingerprint(Some((point.x, point.y)))?;
        point.visual_coordinates()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_remote_android_chrome_uses_visual_coordinates() {
        for (remote, platform, browser, expected) in [
            (true, "Android", "chrome", true),
            (false, "Android", "chrome", false),
            (true, "iOS", "safari", false),
            (true, "Windows", "chrome", false),
            (true, "Android", "firefox", false),
            (true, "Android", "chromium", false),
            (true, "Android", "MicrosoftEdge", false),
            (true, "", "chrome", false),
        ] {
            assert_eq!(
                uses_visual_viewport(remote, &json!({"platformName":platform,"browserName":browser})),
                expected
            );
        }
    }

    #[test]
    fn keyboard_pan_is_subtracted_without_changing_css_pixel_units() {
        let point = ClickPoint {
            x: 200.0,
            y: 389.0,
            offset_x: 20.0,
            offset_y: 172.0,
            width: 300.0,
            height: 434.0,
        };
        assert_eq!(point.visual_coordinates().expect("in viewport"), (180.0, 217.0));
    }

    #[test]
    fn fractional_viewport_round_trips_still_dispatch_integer_native_coordinates() {
        use crate::webdriver::actions::ActionState;
        use crate::{BrowserButton, BrowserModifiers};

        let point = ClickPoint {
            x: 1.4,
            y: 217.4,
            offset_x: 0.4,
            offset_y: 0.4,
            width: 300.0,
            height: 434.0,
        };
        let (x, y) = point.visual_coordinates().expect("visible point");
        let mut payload = ActionState::default().click_payload(x, y, BrowserButton::Left, 1, BrowserModifiers::none());
        use_touch_pointer(&mut payload);
        let movement = &payload["actions"][0]["actions"][0];
        assert_eq!(movement["x"].as_i64(), Some(1));
        assert_eq!(movement["y"].as_i64(), Some(217));
    }

    #[test]
    fn untrusted_points_are_rejected() {
        for y in [f64::NAN, f64::INFINITY, -1.0, 434.0] {
            let point = ClickPoint {
                x: 10.0,
                y,
                offset_x: 0.0,
                offset_y: 0.0,
                width: 300.0,
                height: 434.0,
            };
            assert!(point.visual_coordinates().is_err());
        }
    }

    #[test]
    fn native_touch_keeps_the_actions_and_keyboard_source() {
        let mut payload = json!({"actions":[
            {"type":"key","id":"keyboard","actions":[{"type":"keyUp","value":"x"}]},
            {"type":"pointer","id":"mouse","parameters":{"pointerType":"mouse"},"actions":[
                {"type":"pointerMove","origin":"viewport","x":15,"y":217},
                {"type":"pointerDown","button":0},{"type":"pointerUp","button":0}
            ]}
        ]});
        let keys = payload["actions"][0].clone();
        let actions = payload["actions"][1]["actions"].clone();
        use_touch_pointer(&mut payload);
        assert_eq!(payload["actions"][0], keys);
        assert_eq!(payload["actions"][1]["actions"], actions);
        assert_eq!(payload["actions"][1]["id"], "horizon-touch");
        assert_eq!(payload["actions"][1]["parameters"]["pointerType"], "touch");
    }
}
