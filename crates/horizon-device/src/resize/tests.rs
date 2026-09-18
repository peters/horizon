use super::*;
use crate::{ActRequest, Action, ActionReceipt, Backend, CaptureOptions, Observation, Readiness};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(crate) enum Outcome {
    Confirm,
    Denied,
    Timeout(bool),
    Disconnect,
    Mismatch,
}
pub(crate) struct State {
    width: u32,
    height: u32,
    revision: u32,
    calls: usize,
    supported: bool,
    revisions: bool,
    negotiation_error: bool,
    outcome: Outcome,
}
struct Fake(Arc<Mutex<State>>);
impl Fake {
    fn geometry(&self) -> Geometry {
        let state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Geometry {
            target_id: "fixture".into(),
            surface_id: "display".into(),
            width: state.width,
            height: state.height,
            revision: state.revision.to_string(),
        }
    }
}
impl Backend for Fake {
    fn supports_resize_revisions(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revisions
    }
    fn resize_geometry(&self) -> Result<Geometry> {
        Ok(self.geometry())
    }
    fn doctor(&self) -> Result<Readiness> {
        CaptureOptions::default().plan(&self.geometry())?;
        Ok(Readiness {
            geometry: self.geometry(),
            capabilities: Vec::new(),
            desktop_resize: ResizeReadiness::default(),
        })
    }
    fn screenshot(&self, _: &CaptureOptions) -> Result<Observation> {
        let geometry = self.geometry();
        Ok(Observation {
            source_region: crate::Region {
                x: 0,
                y: 0,
                width: geometry.width,
                height: geometry.height,
            },
            image_dimensions: ImageDimensions {
                width: geometry.width,
                height: geometry.height,
            },
            geometry,
            captured_unix_ms: 0,
            mime_type: "image/png".into(),
            image_base64: String::new(),
        })
    }
    fn act(&mut self, request: &ActRequest) -> Result<ActionReceipt> {
        if request.geometry != self.geometry() {
            return Err(DeviceError::StaleGeometry);
        }
        Ok(ActionReceipt {
            state: "dispatched".into(),
            geometry: self.geometry(),
        })
    }
}
impl ResizeBackend for Fake {
    fn supported(&self) -> Result<bool> {
        let state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.negotiation_error {
            return Err(DeviceError::Unavailable("negotiation failed".into()));
        }
        Ok(state.supported)
    }
    fn dimensions(&self) -> Result<ImageDimensions> {
        let g = self.geometry();
        Ok(ImageDimensions {
            width: g.width,
            height: g.height,
        })
    }
    fn resize(&mut self, requested: ImageDimensions) -> Result<ImageDimensions> {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.calls += 1;
        match state.outcome {
            Outcome::Denied => return Err(DeviceError::ResizeDenied("server rejected".into())),
            Outcome::Timeout(uncertain) => return Err(DeviceError::ResizeTimeout { uncertain }),
            Outcome::Disconnect => return Err(DeviceError::ResizeUncertain("disconnected".into())),
            Outcome::Mismatch => return Ok(ImageDimensions { width: 7, height: 7 }),
            Outcome::Confirm => {}
        }
        state.width = requested.width;
        state.height = requested.height;
        state.revision += 1;
        Ok(requested)
    }
}
pub(crate) fn fixture(enabled: bool, outcome: Outcome) -> (Device, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        width: 1280,
        height: 720,
        revision: 1,
        calls: 0,
        supported: true,
        revisions: true,
        negotiation_error: false,
        outcome,
    }));
    let device = Device {
        backend: Box::new(Fake(Arc::clone(&state))),
        resize: ResizeControl {
            config: ResizeConfig {
                policy: ResizePolicy {
                    enabled,
                    ..Default::default()
                },
                ..Default::default()
            },
            backend: Some(Box::new(Fake(Arc::clone(&state)))),
            uncertain: false,
            needs_observation: std::cell::Cell::new(false),
        },
    };
    (device, state)
}
fn request() -> ResizeRequest {
    ResizeRequest {
        width: 1920,
        height: 1080,
    }
}
#[test]
fn old_configuration_keeps_resize_disabled_and_support_does_not_grant_permission()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let target: crate::Target =
        serde_json::from_str(r#"{"id":"fixture","endpoint":{"kind":"local_x11","display":":99"}}"#)?;
    assert!(!target.desktop_resize.policy.enabled);
    let (mut device, state) = fixture(false, Outcome::Confirm);
    let readiness = device.doctor()?.desktop_resize;
    assert!(readiness.supported);
    assert!(!readiness.permitted);
    assert!(matches!(
        device.resize_desktop(&request()),
        Err(DeviceError::ResizeDenied(_))
    ));
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 0);

    Ok(())
}
#[test]
fn bounds_and_unsupported_requests_never_dispatch() {
    let (mut device, state) = fixture(true, Outcome::Confirm);
    for (width, height) in [(0, 720), (8193, 1), (1, 8193), (4096, 4096)] {
        assert!(matches!(
            device.resize_desktop(&ResizeRequest { width, height }),
            Err(DeviceError::Invalid(_))
        ));
    }
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .supported = false;
    assert!(matches!(
        device.resize_desktop(&request()),
        Err(DeviceError::Unsupported(_))
    ));
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 0);
}
#[test]
fn confirmed_resize_requires_observation_and_rejects_old_coordinates_after_round_trip()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (mut device, _) = fixture(true, Outcome::Confirm);
    let old = device.screenshot()?.geometry;
    let result = device.resize_desktop(&request())?;
    assert_eq!(result.applied, request().dimensions());
    let action = |geometry| ActRequest {
        geometry,
        action: Action::Type {
            text: "synthetic".into(),
        },
    };
    assert!(matches!(
        device.act(&action(result.geometry)),
        Err(DeviceError::StaleGeometry)
    ));
    let fresh = device.screenshot()?.geometry;
    assert!(device.act(&action(fresh)).is_ok());
    device.resize_desktop(&ResizeRequest {
        width: 1280,
        height: 720,
    })?;
    device.screenshot()?;
    assert!(matches!(device.act(&action(old)), Err(DeviceError::StaleGeometry)));

    Ok(())
}
#[test]
fn uncertain_outcomes_block_retries_while_definite_rejections_allow_retry()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    for (outcome, uncertain) in [
        (Outcome::Denied, false),
        (Outcome::Timeout(false), false),
        (Outcome::Timeout(true), true),
        (Outcome::Disconnect, true),
        (Outcome::Mismatch, true),
    ] {
        let (mut device, state) = fixture(true, outcome);
        let error = device
            .resize_desktop(&request())
            .err()
            .ok_or("resize unexpectedly succeeded")?;
        assert_eq!(error.resize_uncertain(), uncertain);
        assert_eq!(device.doctor()?.desktop_resize.uncertain, uncertain);
        state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).outcome = Outcome::Confirm;
        assert_eq!(device.resize_desktop(&request()).is_err(), uncertain);
        assert_eq!(
            state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls,
            if uncertain { 1 } else { 2 }
        );
    }

    Ok(())
}
#[test]
fn unbound_control_crate_remains_unsupported() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let (mut device, state) = fixture(true, Outcome::Confirm);
    device.resize.backend = None;
    assert!(!device.doctor()?.desktop_resize.supported);
    assert!(matches!(
        device.resize_desktop(&request()),
        Err(DeviceError::Unsupported(_))
    ));
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 0);

    Ok(())
}

#[test]
fn a_denied_resize_does_not_erase_an_earlier_screenshot_requirement()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (mut device, state) = fixture(true, Outcome::Confirm);
    let receipt = device.resize_desktop(&request())?;
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).outcome = Outcome::Denied;
    assert!(
        device
            .resize_desktop(&ResizeRequest {
                width: 1280,
                height: 720
            })
            .is_err()
    );
    let action = ActRequest {
        geometry: receipt.geometry,
        action: Action::Type {
            text: "synthetic".into(),
        },
    };
    assert!(matches!(device.act(&action), Err(DeviceError::StaleGeometry)));
    device.screenshot()?;
    assert!(device.act(&action).is_ok());

    Ok(())
}

#[test]
fn owner_limits_cannot_exceed_capture_and_input_bounds() {
    let (mut device, state) = fixture(true, Outcome::Confirm);
    device.resize.config.policy.max_width = u32::MAX;
    device.resize.config.policy.max_height = u32::MAX;
    device.resize.config.policy.max_pixels = u64::MAX;
    for (width, height) in [(32_769, 1), (1, 32_769), (3000, 3000)] {
        assert!(matches!(
            device.resize_desktop(&ResizeRequest { width, height }),
            Err(DeviceError::Invalid(_))
        ));
    }
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 0);
}

#[test]
fn missing_resize_revisions_preserve_observation_and_input() -> Result<()> {
    let (mut device, state) = fixture(true, Outcome::Confirm);
    {
        let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revisions = false;
        state.negotiation_error = true;
    }
    let readiness = device.doctor()?.desktop_resize;
    assert!(readiness.permitted);
    assert!(!readiness.supported);
    assert!(matches!(
        device.resize_desktop(&request()),
        Err(DeviceError::Unsupported(_))
    ));
    let geometry = device.screenshot()?.geometry;
    device.act(&ActRequest {
        geometry,
        action: Action::Type {
            text: "synthetic".into(),
        },
    })?;
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 0);
    Ok(())
}

#[test]
fn confirmed_unchanged_size_does_not_dispatch_to_the_backend() -> Result<()> {
    let (mut device, state) = fixture(true, Outcome::Timeout(true));
    let receipt = device.resize_desktop(&ResizeRequest {
        width: 1280,
        height: 720,
    })?;
    assert_eq!(receipt.requested, receipt.applied);
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 0);
    assert!(!device.resize.uncertain);
    assert!(device.resize.needs_observation.get());
    Ok(())
}

#[test]
fn oversized_current_desktop_can_shrink_without_preflight_capture() -> Result<()> {
    let (mut device, state) = fixture(true, Outcome::Confirm);
    {
        let mut state = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.width = 4096;
        state.height = 2160;
    }
    assert!(matches!(device.doctor(), Err(DeviceError::Invalid(_))));
    let receipt = device.resize_desktop(&request())?;
    assert_eq!(receipt.applied, request().dimensions());
    assert_eq!(state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).calls, 1);
    Ok(())
}
