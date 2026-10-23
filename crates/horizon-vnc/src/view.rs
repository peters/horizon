//! What part of the desktop a viewer shows, how large and how often.
use std::time::Duration;

/// A rectangle of desktop or image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Region {
    /// All of an image or desktop of `size`.
    #[must_use]
    pub const fn whole(size: [usize; 2]) -> Self {
        Self {
            x: 0,
            y: 0,
            width: size[0],
            height: size[1],
        }
    }

    /// The smallest region that holds both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self
            .x
            .saturating_add(self.width)
            .max(other.x.saturating_add(other.width));
        let bottom = self
            .y
            .saturating_add(self.height)
            .max(other.y.saturating_add(other.height));
        Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

/// Crop, size limits and refresh rate for the image a session produces. The
/// desktop resolution and the VNC encoding do not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    pub max_fps: u8,
    pub max_width: usize,
    pub max_height: usize,
    /// The part of the desktop to show; `None` shows all of it.
    pub viewport: Option<Region>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            max_fps: 20,
            max_width: 2048,
            max_height: 2048,
            viewport: None,
        }
    }
}

/// Which desktop pixels an image shows, and the image size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLayout {
    pub source: Region,
    pub output: [usize; 2],
}

impl ImageLayout {
    /// The image is the whole `desktop` at its own size: no crop and no scale.
    #[must_use]
    pub fn shows_all_of(&self, desktop: [usize; 2]) -> bool {
        self.source == Region::whole(desktop) && self.output == desktop
    }
}

/// Options that cannot make an image of the current desktop. The texts are
/// the ones Horizon has always shown and reported, so they keep their prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("Config error: Invalid device refresh rate or image dimensions")]
    Limits,
    #[error("Config error: Viewport must be nonempty and inside the device desktop")]
    Viewport,
}

impl ViewOptions {
    /// Crop and output limits, excluding refresh cadence.
    #[must_use]
    pub fn same_presentation(self, other: Self) -> bool {
        self.max_width == other.max_width && self.max_height == other.max_height && self.viewport == other.viewport
    }

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
    pub fn layout(self, desktop: [usize; 2]) -> Result<ImageLayout, LayoutError> {
        if !(1..=30).contains(&self.max_fps)
            || !(1..=8192).contains(&self.max_width)
            || !(1..=8192).contains(&self.max_height)
            || desktop.contains(&0)
            || desktop[0]
                .checked_mul(desktop[1])
                .is_none_or(|pixels| pixels > 8_294_400)
        {
            return Err(LayoutError::Limits);
        }
        let source = self.viewport.unwrap_or(Region::whole(desktop));
        if source.width == 0
            || source.height == 0
            || source.x.checked_add(source.width).is_none_or(|end| end > desktop[0])
            || source.y.checked_add(source.height).is_none_or(|end| end > desktop[1])
        {
            return Err(LayoutError::Viewport);
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
        Ok(ImageLayout { source, output })
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
    fn rendered_limits_preserve_aspect_without_resizing_source() -> Result<(), LayoutError> {
        let options = ViewOptions {
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
    fn narrow_desktops_are_scaled_with_overflow_safe_pixel_bounds() -> Result<(), LayoutError> {
        let options = ViewOptions::default();
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
            Region {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Region {
                x: 100,
                y: 0,
                width: 1,
                height: 1,
            },
            Region {
                x: usize::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
        ] {
            assert!(
                ViewOptions {
                    viewport: Some(viewport),
                    ..Default::default()
                }
                .layout([100, 80])
                .is_err()
            );
        }
        assert!(
            ViewOptions {
                max_fps: 0,
                ..Default::default()
            }
            .layout([100, 80])
            .is_err()
        );
        assert!(
            ViewOptions {
                max_width: 8193,
                ..Default::default()
            }
            .layout([100, 80])
            .is_err()
        );
    }

    #[test]
    fn presentation_equality_ignores_refresh_cadence() {
        let crop = Region {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let slow = ViewOptions {
            max_fps: 1,
            viewport: Some(crop),
            ..Default::default()
        };
        let fast = ViewOptions {
            max_fps: 30,
            viewport: Some(crop),
            ..Default::default()
        };
        assert!(slow.same_presentation(fast));
        assert!(!slow.same_presentation(ViewOptions { max_width: 100, ..fast }));
        assert!(!slow.same_presentation(ViewOptions {
            max_height: 100,
            ..fast
        }));
        assert!(!slow.same_presentation(ViewOptions { viewport: None, ..fast }));
    }

    #[test]
    fn crop_edges_and_refresh_interval_are_bounded() -> Result<(), LayoutError> {
        let viewport = Region {
            x: 99,
            y: 79,
            width: 1,
            height: 1,
        };
        let options = ViewOptions {
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

    #[test]
    fn a_union_bounds_both_regions() {
        let left = Region {
            x: 1,
            y: 4,
            width: 2,
            height: 1,
        };
        let right = Region {
            x: 5,
            y: 0,
            width: 1,
            height: 2,
        };
        let both = Region {
            x: 1,
            y: 0,
            width: 5,
            height: 5,
        };
        assert_eq!(left.union(right), both);
        assert_eq!(right.union(left), both);
        assert_eq!(both.union(left), both, "a region inside adds nothing");
        assert_eq!(
            Region::whole([3, 2]).union(Region::whole([3, 2])),
            Region::whole([3, 2])
        );
    }
}
