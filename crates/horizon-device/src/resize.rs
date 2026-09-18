//! Owner-controlled desktop resizing, independent of any transport implementation.
use crate::{Device, DeviceError, Geometry, ImageDimensions, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResizePolicy {
    pub enabled: bool,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
}
impl Default for ResizePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_width: 8192,
            max_height: 8192,
            max_pixels: 8_294_400,
        }
    }
}
impl ResizePolicy {
    fn validate(&self, size: ImageDimensions) -> Result<()> {
        if !self.enabled {
            return Err(DeviceError::ResizeDenied(
                "desktop resizing is disabled by the target owner".into(),
            ));
        }
        if size.width == 0
            || size.height == 0
            || size.width > self.max_width
            || size.height > self.max_height
            || u64::from(size.width) * u64::from(size.height) > self.max_pixels
        {
            return Err(DeviceError::Invalid("desktop size exceeds target limits".into()));
        }
        Ok(())
    }
}

/// The owner binds this loopback VNC server to the target's local X11 session.
/// An endpoint advertises support; only `policy.enabled` grants permission.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResizeConfig {
    pub policy: ResizePolicy,
    pub vnc_address: Option<SocketAddr>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResizeReadiness {
    pub permitted: bool,
    pub supported: bool,
    pub uncertain: bool,
    pub limits: ResizePolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "cli", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ResizeRequest {
    pub width: u32,
    pub height: u32,
}
impl ResizeRequest {
    pub(crate) fn dimensions(&self) -> ImageDimensions {
        ImageDimensions {
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResizeReceipt {
    pub requested: ImageDimensions,
    pub applied: ImageDimensions,
    pub geometry: Geometry,
}

/// A synchronous, bounded resize transport. Implementations must retain screen
/// identity, confirm server outcomes, and never return success on dispatch alone.
/// A timeout after dispatch must set `uncertain: true`; disconnect or mismatched
/// confirmation after dispatch must return `ResizeUncertain`.
pub trait ResizeBackend {
    /// # Errors
    /// Returns an error if capability negotiation cannot complete within its bound.
    fn supported(&self) -> Result<bool>;
    /// # Errors
    /// Returns an error if current server geometry cannot be established.
    fn dimensions(&self) -> Result<ImageDimensions>;
    /// # Errors
    /// Returns a typed rejection, timeout or uncertain mutation outcome.
    fn resize(&mut self, requested: ImageDimensions) -> Result<ImageDimensions>;
}

pub(crate) struct ResizeControl {
    pub config: ResizeConfig,
    pub backend: Option<Box<dyn ResizeBackend>>,
    pub uncertain: bool,
    pub needs_observation: std::cell::Cell<bool>,
}
impl ResizeControl {
    pub fn readiness(&self) -> Result<ResizeReadiness> {
        Ok(ResizeReadiness {
            permitted: self.config.policy.enabled,
            supported: match &self.backend {
                Some(backend) => backend.supported()?,
                None => false,
            },
            uncertain: self.uncertain,
            limits: self.config.policy.clone(),
        })
    }
}

impl Device {
    /// Install a transport for the session explicitly bound by the target owner.
    /// This does not enable the target's resize permission.
    #[must_use]
    pub fn with_resize_backend(mut self, backend: Box<dyn ResizeBackend>) -> Self {
        self.resize.backend = Some(backend);
        self
    }

    /// Resize the actual desktop, then obtain a fresh screenshot before input.
    /// The caller serializes operations and must preserve uncertain outcomes
    /// across reconnections. The CLI/MCP runner journals these before dispatch.
    ///
    /// # Errors
    /// Rejects disabled/unsupported/oversized requests before mutation. A timeout
    /// or disconnect after dispatch blocks further resizing on this device.
    pub fn resize_desktop(&mut self, request: &ResizeRequest) -> Result<ResizeReceipt> {
        let requested = request.dimensions();
        self.resize.config.policy.validate(requested)?;
        if self.resize.uncertain {
            return Err(DeviceError::ResizeUncertain(
                "previous resize requires owner reconciliation".into(),
            ));
        }
        let backend = self
            .resize
            .backend
            .as_mut()
            .ok_or_else(|| DeviceError::Unsupported("no desktop resize adapter".into()))?;
        if !backend.supported()? {
            return Err(DeviceError::Unsupported(
                "server does not support desktop resizing".into(),
            ));
        }
        let before = self.backend.doctor()?.geometry;
        if backend.dimensions()?
            != (ImageDimensions {
                width: before.width,
                height: before.height,
            })
        {
            return Err(DeviceError::Invalid(
                "resize server and input surface dimensions differ".into(),
            ));
        }
        let needs_observation = self.resize.needs_observation.get();
        self.resize.uncertain = true;
        self.resize.needs_observation.set(true);
        let applied = match backend.resize(requested) {
            Ok(applied) => applied,
            Err(error) => {
                self.resize.uncertain = error.resize_uncertain();
                self.resize
                    .needs_observation
                    .set(needs_observation || self.resize.uncertain);
                return Err(error);
            }
        };
        let geometry = self
            .backend
            .doctor()
            .map_err(|error| DeviceError::ResizeUncertain(error.to_string()))?
            .geometry;
        if applied != requested || (geometry.width, geometry.height) != (applied.width, applied.height) {
            return Err(DeviceError::ResizeUncertain(
                "confirmed desktop and input surface dimensions differ".into(),
            ));
        }
        self.resize.uncertain = false;
        Ok(ResizeReceipt {
            requested,
            applied,
            geometry,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
