//! The desktop as the server draws it: decoded rectangles composited into one
//! RGBA buffer, with the bounds of what changed since the last image.
use vnc::{Rect, VncEvent};

use crate::{Error, Frame, Region};

/// The most desktop pixels a framebuffer holds (3840 × 2160).
const MAX_PIXELS: usize = 8_294_400;

#[derive(Default)]
pub(crate) struct Framebuffer {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
    /// Bounds every pixel written since the last `take_damage`.
    damage: Option<Region>,
}

impl Framebuffer {
    pub(crate) fn apply(&mut self, event: VncEvent) -> Result<bool, Error> {
        match event {
            VncEvent::SetResolution(size) => self.resize(usize::from(size.width), usize::from(size.height)),
            VncEvent::DesktopUpdate(update) => {
                let Some(layout) = update.layout else {
                    return Ok(false);
                };
                let (width, height) = (usize::from(layout.width), usize::from(layout.height));
                if self.size() == [width, height] {
                    return Ok(false);
                }
                self.resize(width, height)
            }
            VncEvent::RawImage(rect, bytes) => {
                self.write(rect, &bytes)?;
                Ok(true)
            }
            VncEvent::Copy(destination, source) => {
                self.check(source)?;
                self.check(destination)?;
                if source.width != destination.width || source.height != destination.height {
                    return Err(Error::Frame("copy dimensions differ"));
                }
                // Snapshot the source so overlapping copies have memmove semantics.
                let mut bytes = Vec::with_capacity(usize::from(source.width) * usize::from(source.height) * 4);
                for row in 0..usize::from(source.height) {
                    let start = ((usize::from(source.y) + row) * self.width + usize::from(source.x)) * 4;
                    bytes.extend_from_slice(&self.rgba[start..start + usize::from(source.width) * 4]);
                }
                self.write(destination, &bytes)?;
                Ok(true)
            }
            VncEvent::Error(error) => Err(Error::Server(error)),
            VncEvent::JpegImage(_, _) | VncEvent::SetCursor(_, _) => Err(Error::Frame("unrequested image encoding")),
            VncEvent::SetPixelFormat(_) | VncEvent::Bell | VncEvent::Text(_) => Ok(false),
            _ => Err(Error::Frame("unsupported viewer event")),
        }
    }

    fn resize(&mut self, width: usize, height: usize) -> Result<bool, Error> {
        if width == 0 || height == 0 || width * height > MAX_PIXELS {
            return Err(Error::Frame("unsupported desktop size"));
        }
        self.width = width;
        self.height = height;
        self.rgba = vec![0; width * height * 4];
        self.damage = Some(Region::whole([width, height]));
        Ok(true)
    }

    fn check(&self, rect: Rect) -> Result<(), Error> {
        if usize::from(rect.x) + usize::from(rect.width) > self.width
            || usize::from(rect.y) + usize::from(rect.height) > self.height
        {
            return Err(Error::Frame("rectangle outside desktop"));
        }
        Ok(())
    }

    fn write(&mut self, rect: Rect, bytes: &[u8]) -> Result<(), Error> {
        self.check(rect)?;
        let row_bytes = usize::from(rect.width) * 4;
        if bytes.len() != row_bytes * usize::from(rect.height) {
            return Err(Error::Frame("rectangle byte count mismatch"));
        }
        let written = Region {
            x: usize::from(rect.x),
            y: usize::from(rect.y),
            width: usize::from(rect.width),
            height: usize::from(rect.height),
        };
        self.damage = Some(self.damage.map_or(written, |damage| damage.union(written)));
        for row in 0..usize::from(rect.height) {
            let start = ((usize::from(rect.y) + row) * self.width + usize::from(rect.x)) * 4;
            let destination = &mut self.rgba[start..start + row_bytes];
            destination.copy_from_slice(&bytes[row * row_bytes..(row + 1) * row_bytes]);
            for pixel in destination.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
        Ok(())
    }

    pub(crate) fn size(&self) -> [usize; 2] {
        [self.width, self.height]
    }

    /// A copy of the whole desktop.
    pub(crate) fn frame(&self) -> Result<Frame, Error> {
        if self.size().contains(&0) {
            return Err(Error::Frame("unsupported desktop size"));
        }
        Frame::from_rgba(self.size(), &self.rgba)
    }

    /// The bounds of every pixel written since the last call; `None` when
    /// nothing was. A resize counts as a write of the whole desktop.
    pub(crate) fn take_damage(&mut self) -> Option<Region> {
        self.damage.take()
    }

    #[cfg(test)]
    pub(crate) fn image(&self, options: crate::ViewOptions) -> Result<Frame, Error> {
        self.frame()?.present(options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ViewOptions;

    fn reds(frame: &Frame) -> Vec<u8> {
        frame.rgba().as_chunks::<4>().0.iter().map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn unchanged_extended_layout_preserves_pixels_and_rejection_preserves_geometry() -> Result<(), Error> {
        let mut frame = Framebuffer::default();
        frame.resize(2, 1)?;
        frame.write(
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            &[20, 180, 40, 0],
        )?;
        let before = frame.rgba.clone();
        let update = vnc::DesktopUpdate {
            reason: vnc::DesktopReason::Server,
            status: vnc::DesktopStatus::Success,
            layout: Some(vnc::DesktopLayout {
                width: 2,
                height: 1,
                screens: Vec::new(),
            }),
        };
        assert!(!frame.apply(VncEvent::DesktopUpdate(update))?);
        assert_eq!(frame.rgba, before);
        assert!(!frame.apply(VncEvent::DesktopUpdate(vnc::DesktopUpdate {
            reason: vnc::DesktopReason::ThisClient,
            status: vnc::DesktopStatus::Prohibited,
            layout: None,
        }))?);
        assert_eq!(frame.size(), [2, 1]);
        assert_eq!(frame.rgba, before);
        Ok(())
    }

    #[test]
    fn overlapping_copy_and_resize_preserve_valid_pixels() -> Result<(), Error> {
        let mut frame = Framebuffer::default();
        frame.apply(VncEvent::SetResolution(vnc::Screen { width: 3, height: 1 }))?;
        frame.apply(VncEvent::RawImage(
            Rect {
                x: 0,
                y: 0,
                width: 3,
                height: 1,
            },
            vec![1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0],
        ))?;
        frame.apply(VncEvent::Copy(
            Rect {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            },
            Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            },
        ))?;
        assert_eq!(frame.rgba, [1, 2, 3, 255, 1, 2, 3, 255, 4, 5, 6, 255]);
        frame.apply(VncEvent::SetResolution(vnc::Screen { width: 1, height: 2 }))?;
        assert_eq!(frame.image(ViewOptions::default())?.size(), [1, 2]);
        assert_eq!(frame.rgba, [0; 8]);
        Ok(())
    }

    #[test]
    fn viewport_scaling_samples_the_selected_source_pixels() -> Result<(), Error> {
        let mut frame = Framebuffer::default();
        frame.apply(VncEvent::SetResolution(vnc::Screen { width: 4, height: 2 }))?;
        frame.apply(VncEvent::RawImage(
            Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 2,
            },
            (0..8).flat_map(|value| [value, 0, 0, 255]).collect(),
        ))?;
        let identity = frame.image(ViewOptions::default())?;
        assert_eq!(identity.size(), [4, 2]);
        assert_eq!(reds(&identity), (0..8).collect::<Vec<_>>());
        let options = ViewOptions {
            viewport: Some(Region {
                x: 2,
                y: 0,
                width: 2,
                height: 2,
            }),
            max_width: 1,
            max_height: 1,
            ..Default::default()
        };
        let image = frame.image(options)?;
        assert_eq!(image.size(), [1, 1]);
        assert_eq!(reds(&image), [7]);
        assert_eq!(frame.size(), [4, 2]);
        frame.apply(VncEvent::SetResolution(vnc::Screen { width: 2, height: 2 }))?;
        assert_eq!(frame.image(options)?.size(), [1, 1]);
        let held = Frame::from_rgba([4, 2], &(0..8).flat_map(|value| [value, 0, 0, 255]).collect::<Vec<_>>())?;
        let presented = held.present(options)?;
        assert_eq!(presented.size(), [1, 1]);
        assert_eq!(reds(&presented), [7]);
        Ok(())
    }

    #[test]
    fn malformed_rectangles_and_oversized_desktops_fail() {
        let mut frame = Framebuffer::default();
        assert!(
            frame
                .apply(VncEvent::RawImage(
                    Rect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1
                    },
                    vec![0; 4]
                ))
                .is_err()
        );
        assert!(
            frame
                .apply(VncEvent::SetResolution(vnc::Screen {
                    width: u16::MAX,
                    height: u16::MAX
                }))
                .is_err()
        );
    }

    #[test]
    fn damage_bounds_every_write_since_it_was_taken() -> Result<(), Error> {
        let mut frame = Framebuffer::default();
        assert_eq!(frame.take_damage(), None);
        frame.apply(VncEvent::SetResolution(vnc::Screen { width: 4, height: 3 }))?;
        assert_eq!(
            frame.take_damage(),
            Some(Region::whole([4, 3])),
            "a resize redraws everything"
        );
        assert_eq!(frame.take_damage(), None);
        let pixel = |x, y| {
            VncEvent::RawImage(
                Rect {
                    x,
                    y,
                    width: 1,
                    height: 1,
                },
                vec![9; 4],
            )
        };
        frame.apply(pixel(1, 0))?;
        frame.apply(pixel(3, 1))?;
        assert_eq!(
            frame.take_damage(),
            Some(Region {
                x: 1,
                y: 0,
                width: 3,
                height: 2,
            })
        );
        frame.apply(VncEvent::Copy(
            Rect {
                x: 0,
                y: 2,
                width: 2,
                height: 1,
            },
            Rect {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            },
        ))?;
        assert_eq!(
            frame.take_damage(),
            Some(Region {
                x: 0,
                y: 2,
                width: 2,
                height: 1,
            }),
            "a copy changes its destination only"
        );
        assert!(!frame.apply(VncEvent::Bell)?);
        assert_eq!(frame.take_damage(), None, "events without pixels change nothing");
        Ok(())
    }
}
