use std::time::Duration;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceViewport {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceViewOptions {
    pub max_fps: u8,
    pub max_width: usize,
    pub max_height: usize,
    pub viewport: Option<DeviceViewport>,
}

impl Default for DeviceViewOptions {
    fn default() -> Self {
        Self {
            max_fps: 20,
            max_width: 2048,
            max_height: 2048,
            viewport: None,
        }
    }
}

pub struct DeviceImageLayout {
    pub source: DeviceViewport,
    pub output: [usize; 2],
}

impl DeviceViewOptions {
    /// Return to the whole desktop when a resize makes the active crop invalid.
    #[must_use]
    pub fn for_desktop(mut self, desktop: [usize; 2]) -> Self {
        if self.viewport.is_some_and(|viewport| {
            viewport.width == 0
                || viewport.height == 0
                || viewport
                    .x
                    .checked_add(viewport.width)
                    .is_none_or(|end| end > desktop[0])
                || viewport
                    .y
                    .checked_add(viewport.height)
                    .is_none_or(|end| end > desktop[1])
        }) {
            self.viewport = None;
        }
        self
    }

    /// # Errors
    /// Rejects invalid limits and viewports outside the current desktop.
    pub fn layout(self, desktop: [usize; 2]) -> Result<DeviceImageLayout> {
        if !(1..=30).contains(&self.max_fps)
            || !(1..=8192).contains(&self.max_width)
            || !(1..=8192).contains(&self.max_height)
            || desktop.contains(&0)
            || desktop[0]
                .checked_mul(desktop[1])
                .is_none_or(|pixels| pixels > 8_294_400)
        {
            return Err(Error::Config("Invalid device refresh rate or image dimensions".into()));
        }
        let source = self.viewport.unwrap_or(DeviceViewport {
            x: 0,
            y: 0,
            width: desktop[0],
            height: desktop[1],
        });
        if source.width == 0
            || source.height == 0
            || source.x.checked_add(source.width).is_none_or(|end| end > desktop[0])
            || source.y.checked_add(source.height).is_none_or(|end| end > desktop[1])
        {
            return Err(Error::Config(
                "Viewport must be nonempty and inside the device desktop".into(),
            ));
        }
        let mut output = [source.width, source.height];
        if output[0] > self.max_width {
            output[1] = (output[1] * self.max_width / output[0]).max(1);
            output[0] = self.max_width;
        }
        if output[1] > self.max_height {
            output[0] = (output[0] * self.max_height / output[1]).max(1);
            output[1] = self.max_height;
        }
        Ok(DeviceImageLayout { source, output })
    }

    #[must_use]
    pub fn interval(self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.max_fps.clamp(1, 30)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_limits_preserve_aspect_without_resizing_source() -> Result<()> {
        let options = DeviceViewOptions {
            max_width: 800,
            max_height: 300,
            ..Default::default()
        };
        let image = options.layout([1600, 1000])?;
        assert_eq!(image.output, [480, 300]);
        assert_eq!((image.source.width, image.source.height), (1600, 1000));
        let image = options.layout([10, 8])?;
        assert_eq!(image.output, [10, 8]);
        Ok(())
    }

    #[test]
    fn narrow_desktops_are_scaled_with_overflow_safe_pixel_bounds() -> Result<()> {
        let options = DeviceViewOptions::default();
        assert_eq!(options.layout([10_000, 100])?.output, [2048, 20]);
        assert_eq!(options.layout([100, 10_000])?.output, [20, 2048]);
        for desktop in [[usize::MAX, 2], [usize::MAX, usize::MAX], [0, 100], [8192, 8192]] {
            assert!(options.layout(desktop).is_err());
        }
        Ok(())
    }

    #[test]
    fn invalid_limits_and_outside_viewports_fail() {
        for viewport in [
            DeviceViewport {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            DeviceViewport {
                x: 100,
                y: 0,
                width: 1,
                height: 1,
            },
            DeviceViewport {
                x: usize::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
        ] {
            assert!(
                DeviceViewOptions {
                    viewport: Some(viewport),
                    ..Default::default()
                }
                .layout([100, 80])
                .is_err()
            );
        }
        assert!(
            DeviceViewOptions {
                max_fps: 0,
                ..Default::default()
            }
            .layout([100, 80])
            .is_err()
        );
        assert!(
            DeviceViewOptions {
                max_width: 8193,
                ..Default::default()
            }
            .layout([100, 80])
            .is_err()
        );
    }

    #[test]
    fn crop_edges_and_refresh_interval_are_bounded() -> Result<()> {
        let viewport = DeviceViewport {
            x: 99,
            y: 79,
            width: 1,
            height: 1,
        };
        let options = DeviceViewOptions {
            max_fps: 1,
            viewport: Some(viewport),
            ..Default::default()
        };
        let image = options.layout([100, 80])?;
        assert_eq!(image.source, viewport);
        assert_eq!(image.output, [1, 1]);
        assert_eq!(options.interval(), Duration::from_secs(1));
        Ok(())
    }
}
