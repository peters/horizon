#![forbid(unsafe_code)]
//! Device observation and bounded input. The initial backend is local Linux X11.
//! No GUI, model provider, application lifecycle, or transport is required.

mod capture;
#[cfg(feature = "cli")]
pub mod cli;
mod model;
mod resize;
pub use resize::*;
#[cfg(target_os = "linux")]
mod x11;
pub use model::*;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("unsupported capability or backend: {0}")]
    Unsupported(String),
    #[error("target unavailable: {0}")]
    Unavailable(String),
    #[error("geometry changed; obtain a new screenshot")]
    StaleGeometry,
    #[error("desktop resize denied: {0}")]
    ResizeDenied(String),
    #[error("desktop resize timed out (uncertain: {uncertain})")]
    ResizeTimeout { uncertain: bool },
    #[error("desktop resize outcome uncertain; owner reconciliation required: {0}")]
    ResizeUncertain(String),
    #[error("input outcome indeterminate; observe before retrying: {0}")]
    Indeterminate(String),
}
impl DeviceError {
    #[must_use]
    pub const fn resize_uncertain(&self) -> bool {
        matches!(self, Self::ResizeUncertain(_) | Self::ResizeTimeout { uncertain: true })
    }
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_request",
            Self::Unsupported(_) => "unsupported",
            Self::Unavailable(_) => "unavailable",
            Self::StaleGeometry => "stale_geometry",
            Self::ResizeDenied(_) => "resize_denied",
            Self::ResizeTimeout { .. } => "resize_timeout",
            Self::ResizeUncertain(_) => "resize_uncertain",
            Self::Indeterminate(_) => "indeterminate",
        }
    }
}
pub type Result<T> = std::result::Result<T, DeviceError>;

/// A small backend boundary; remote/mobile implementations need no native handles
/// in public requests. Backends must validate before sending any input.
trait Backend {
    fn supports_resize_revisions(&self) -> bool {
        false
    }
    fn doctor(&self) -> Result<Readiness>;
    fn screenshot(&self, options: &CaptureOptions) -> Result<Observation>;
    fn act(&mut self, request: &ActRequest) -> Result<ActionReceipt>;
}

pub struct Device {
    backend: Box<dyn Backend>,
    resize: resize::ResizeControl,
}
impl Device {
    /// Open an explicit target. The caller serializes access to shared device input.
    ///
    /// # Errors
    /// Rejects invalid, unsupported, or unavailable targets.
    pub fn connect(target: &Target) -> Result<Self> {
        if target.id.is_empty() || target.id.len() > 128 {
            return Err(DeviceError::Invalid("target id must contain 1..128 bytes".into()));
        }
        #[cfg(target_os = "linux")]
        {
            Ok(Self {
                backend: Box::new(x11::X11::connect(target)?),
                resize: resize::ResizeControl {
                    config: target.desktop_resize.clone(),
                    backend: None,
                    uncertain: false,
                    needs_observation: std::cell::Cell::new(false),
                },
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(DeviceError::Unsupported("only local X11 is implemented".into()))
        }
    }
    /// # Errors
    /// Returns an error when capture geometry or input readiness cannot be queried.
    pub fn doctor(&self) -> Result<Readiness> {
        let mut readiness = self.backend.doctor()?;
        readiness.desktop_resize = self.resize.readiness()?;
        readiness.desktop_resize.supported &= self.backend.supports_resize_revisions();
        Ok(readiness)
    }
    /// # Errors
    /// Returns an error for unavailable capture, unsupported formats, or geometry changes.
    pub fn screenshot(&self) -> Result<Observation> {
        self.screenshot_with(&CaptureOptions::default())
    }
    /// Capture a bounded crop and optional scaled PNG/JPEG image.
    ///
    /// # Errors
    /// Rejects invalid capture options before reading pixels.
    pub fn screenshot_with(&self, options: &CaptureOptions) -> Result<Observation> {
        let observation = self.backend.screenshot(options)?;
        self.resize.needs_observation.set(false);
        Ok(observation)
    }
    /// # Errors
    /// Rejects invalid/stale requests before input; partial input returns indeterminate.
    pub fn act(&mut self, request: &ActRequest) -> Result<ActionReceipt> {
        if self.resize.needs_observation.get() {
            return Err(DeviceError::StaleGeometry);
        }
        validate(request)?;
        self.backend.act(request)
    }
}

fn validate(request: &ActRequest) -> Result<()> {
    let g = &request.geometry;
    let point = |p: &Point| -> Result<()> {
        if p.x < 0 || p.y < 0 || p.x.cast_unsigned() >= g.width || p.y.cast_unsigned() >= g.height {
            return Err(DeviceError::Invalid("point outside observed surface".into()));
        }
        Ok(())
    };
    match &request.action {
        Action::Click { at, .. } => point(at)?,
        Action::Drag { from, to, duration_ms } => {
            point(from)?;
            point(to)?;
            if !(1..=2000).contains(duration_ms) {
                return Err(DeviceError::Invalid("drag duration must be 1..2000 ms".into()));
            }
        }
        Action::Scroll {
            at,
            vertical_notches,
            horizontal_notches,
        } => {
            point(at)?;
            if vertical_notches.unsigned_abs() > 100 || horizontal_notches.unsigned_abs() > 100 {
                return Err(DeviceError::Invalid("scroll limited to 100 notches".into()));
            }
        }
        Action::Type { text } if text.len() > 4096 || text.chars().count() > 256 || text.contains('\0') => {
            return Err(DeviceError::Invalid(
                "text limited to 256 Unicode scalars and 4096 UTF-8 bytes, without NUL".into(),
            ));
        }
        Action::Key { modifiers, .. } if modifiers.len() > 4 => {
            return Err(DeviceError::Invalid("at most four modifiers".into()));
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
