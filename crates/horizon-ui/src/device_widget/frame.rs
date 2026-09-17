use egui::ColorImage;
use vnc::{Rect, VncEvent};

use super::session::ViewError;

#[derive(Default)]
pub(super) struct Framebuffer {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Framebuffer {
    pub(super) fn apply(&mut self, event: VncEvent) -> Result<bool, ViewError> {
        match event {
            VncEvent::SetResolution(size) => {
                let (width, height) = (usize::from(size.width), usize::from(size.height));
                if width == 0 || height == 0 || width * height > 8_294_400 {
                    return Err(ViewError::Frame("unsupported desktop size"));
                }
                self.width = width;
                self.height = height;
                self.rgba = vec![0; width * height * 4];
                Ok(true)
            }
            VncEvent::RawImage(rect, bytes) => {
                self.write(rect, &bytes)?;
                Ok(true)
            }
            VncEvent::Copy(destination, source) => {
                self.check(source)?;
                self.check(destination)?;
                if source.width != destination.width || source.height != destination.height {
                    return Err(ViewError::Frame("copy dimensions differ"));
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
            VncEvent::Error(error) => Err(ViewError::Server(error)),
            VncEvent::JpegImage(_, _) | VncEvent::SetCursor(_, _) => {
                Err(ViewError::Frame("unrequested image encoding"))
            }
            VncEvent::SetPixelFormat(_) | VncEvent::Bell | VncEvent::Text(_) => Ok(false),
            _ => Err(ViewError::Frame("unsupported viewer event")),
        }
    }

    fn check(&self, rect: Rect) -> Result<(), ViewError> {
        if usize::from(rect.x) + usize::from(rect.width) > self.width
            || usize::from(rect.y) + usize::from(rect.height) > self.height
        {
            return Err(ViewError::Frame("rectangle outside desktop"));
        }
        Ok(())
    }

    fn write(&mut self, rect: Rect, bytes: &[u8]) -> Result<(), ViewError> {
        self.check(rect)?;
        let row_bytes = usize::from(rect.width) * 4;
        if bytes.len() != row_bytes * usize::from(rect.height) {
            return Err(ViewError::Frame("rectangle byte count mismatch"));
        }
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

    pub(super) fn size(&self) -> [usize; 2] {
        [self.width, self.height]
    }

    pub(super) fn image(&self, options: horizon_core::DeviceViewOptions) -> Result<ColorImage, ViewError> {
        let layout = options
            .layout(self.size())
            .map_err(|error| ViewError::Server(error.to_string()))?;
        if layout.output == self.size() {
            return Ok(ColorImage::from_rgba_unmultiplied(self.size(), &self.rgba));
        }
        let source_columns: Vec<_> = (0..layout.output[0])
            .map(|x| (layout.source.x + (2 * x + 1) * layout.source.width / (2 * layout.output[0])) * 4)
            .collect();
        let mut pixels = Vec::with_capacity(layout.output[0] * layout.output[1]);
        for y in 0..layout.output[1] {
            let row = (layout.source.y + (2 * y + 1) * layout.source.height / (2 * layout.output[1])) * self.width * 4;
            for column in &source_columns {
                let offset = row + column;
                pixels.push(egui::Color32::from_rgba_unmultiplied(
                    self.rgba[offset],
                    self.rgba[offset + 1],
                    self.rgba[offset + 2],
                    self.rgba[offset + 3],
                ));
            }
        }
        Ok(ColorImage::new(layout.output, pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_copy_and_resize_preserve_valid_pixels() -> Result<(), ViewError> {
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
        assert_eq!(frame.image(horizon_core::DeviceViewOptions::default())?.size, [1, 2]);
        assert_eq!(frame.rgba, [0; 8]);
        Ok(())
    }

    #[test]
    fn viewport_scaling_samples_the_selected_source_pixels() -> Result<(), ViewError> {
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
        let identity = frame.image(horizon_core::DeviceViewOptions::default())?;
        assert_eq!(identity.size, [4, 2]);
        assert_eq!(
            identity.pixels.iter().map(egui::Color32::r).collect::<Vec<_>>(),
            (0..8).collect::<Vec<_>>()
        );
        let options = horizon_core::DeviceViewOptions {
            viewport: Some(horizon_core::DeviceViewport {
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
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.pixels[0].r(), 7);
        assert_eq!(frame.size(), [4, 2]);
        frame.apply(VncEvent::SetResolution(vnc::Screen { width: 2, height: 2 }))?;
        assert!(
            frame.image(options).is_err(),
            "a stale viewport is refused after target resize"
        );
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
}
