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

    pub(super) fn image(&self) -> ColorImage {
        ColorImage::from_rgba_unmultiplied([self.width, self.height], &self.rgba)
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
        assert_eq!(frame.image().size, [1, 2]);
        assert_eq!(frame.rgba, [0; 8]);
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
