use crate::{CaptureOptions, DeviceError, Geometry, ImageDimensions, ImageFormat, Observation, Point, Region, Result};

const MAX_PIXELS: u64 = 8_294_400;
const MAX_IMAGE_SIDE: u32 = 32768;
#[cfg(any(target_os = "linux", test))]
const DEFAULT_JPEG_QUALITY: u8 = 85;

pub(crate) struct CapturePlan {
    pub region: Region,
    pub output: ImageDimensions,
    #[cfg(any(target_os = "linux", test))]
    format: ImageFormat,
    #[cfg(any(target_os = "linux", test))]
    quality: u8,
}

impl CaptureOptions {
    pub(crate) fn plan(&self, geometry: &Geometry) -> Result<CapturePlan> {
        let region = self.region.unwrap_or(Region {
            x: 0,
            y: 0,
            width: geometry.width,
            height: geometry.height,
        });
        if region.width > MAX_IMAGE_SIDE
            || region.height > MAX_IMAGE_SIDE
            || region.width == 0
            || region.height == 0
            || region
                .x
                .checked_add(region.width)
                .is_none_or(|end| end > geometry.width)
            || region
                .y
                .checked_add(region.height)
                .is_none_or(|end| end > geometry.height)
            || u64::from(region.width) * u64::from(region.height) > MAX_PIXELS
        {
            return Err(DeviceError::Invalid(
                "crop must be nonempty, inside the surface and at most 8294400 pixels".into(),
            ));
        }
        let output = self.output.unwrap_or(ImageDimensions {
            width: region.width,
            height: region.height,
        });
        if output.width == 0
            || output.height == 0
            || output.width > MAX_IMAGE_SIDE
            || output.height > MAX_IMAGE_SIDE
            || u64::from(output.width) * u64::from(output.height) > MAX_PIXELS
        {
            return Err(DeviceError::Invalid(
                "image dimensions must be 1..32768 and at most 8294400 pixels".into(),
            ));
        }
        if self.quality.is_some_and(|value| !(1..=100).contains(&value))
            || (self.quality.is_some() && self.format == ImageFormat::Png)
        {
            return Err(DeviceError::Invalid("quality requires JPEG and must be 1..100".into()));
        }
        Ok(CapturePlan {
            region,
            output,
            #[cfg(any(target_os = "linux", test))]
            format: self.format,
            #[cfg(any(target_os = "linux", test))]
            quality: self.quality.unwrap_or(DEFAULT_JPEG_QUALITY),
        })
    }
}

#[cfg(any(target_os = "linux", test))]
impl CapturePlan {
    pub fn encode(&self, rgb: Vec<u8>) -> Result<(Vec<u8>, &'static str)> {
        if rgb.len() != self.region.width as usize * self.region.height as usize * 3 {
            return Err(DeviceError::Unavailable("capture byte count mismatch".into()));
        }
        let pixels = if self.region.width == self.output.width && self.region.height == self.output.height {
            rgb
        } else {
            let mut resized = Vec::with_capacity(self.output.width as usize * self.output.height as usize * 3);
            for y in 0..self.output.height {
                let sy = sample(y, self.output.height, self.region.height);
                for x in 0..self.output.width {
                    let sx = sample(x, self.output.width, self.region.width);
                    let offset = (sy as usize * self.region.width as usize + sx as usize) * 3;
                    resized.extend_from_slice(&rgb[offset..offset + 3]);
                }
            }
            resized
        };
        let mut bytes = Vec::new();
        let mime = match self.format {
            ImageFormat::Png => {
                let mut encoder = png::Encoder::new(&mut bytes, self.output.width, self.output.height);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .map_err(encode_error)?
                    .write_image_data(&pixels)
                    .map_err(encode_error)?;
                "image/png"
            }
            ImageFormat::Jpeg => {
                jpeg_encoder::Encoder::new(&mut bytes, self.quality)
                    .encode(
                        &pixels,
                        u16::try_from(self.output.width).map_err(encode_error)?,
                        u16::try_from(self.output.height).map_err(encode_error)?,
                        jpeg_encoder::ColorType::Rgb,
                    )
                    .map_err(encode_error)?;
                "image/jpeg"
            }
        };
        Ok((bytes, mime))
    }
}

// Pixel-center nearest-neighbor mapping is shared by resizing and input mapping.
fn sample(value: u32, output: u32, source: u32) -> u32 {
    // All three values are bounded to 32768 by the capture plan.
    ((value * 2 + 1) * source) / (output * 2)
}
#[cfg(any(target_os = "linux", test))]
fn encode_error(error: impl std::fmt::Display) -> DeviceError {
    DeviceError::Unavailable(error.to_string())
}

impl Observation {
    /// Map an image pixel to its sampled point on the original device surface.
    /// Use the unchanged observation geometry with the resulting input action.
    ///
    /// # Errors
    /// Rejects image pixels outside the image or invalid observation metadata.
    pub fn surface_point(&self, point: &Point) -> Result<Point> {
        let options = CaptureOptions {
            region: Some(self.source_region),
            output: Some(self.image_dimensions),
            ..Default::default()
        };
        let plan = options.plan(&self.geometry)?;
        if point.x < 0
            || point.y < 0
            || point.x.cast_unsigned() >= plan.output.width
            || point.y.cast_unsigned() >= plan.output.height
        {
            return Err(DeviceError::Invalid("point outside captured image".into()));
        }
        let x = plan.region.x + sample(point.x.cast_unsigned(), plan.output.width, plan.region.width);
        let y = plan.region.y + sample(point.y.cast_unsigned(), plan.output.height, plan.region.height);
        Ok(Point {
            x: i32::try_from(x).map_err(|_| DeviceError::Invalid("surface x exceeds input range".into()))?,
            y: i32::try_from(y).map_err(|_| DeviceError::Invalid("surface y exceeds input range".into()))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    fn geometry() -> Geometry {
        Geometry {
            target_id: "synthetic".into(),
            surface_id: "display".into(),
            width: 4,
            height: 3,
            revision: "one".into(),
        }
    }

    #[test]
    fn crop_limits_are_checked_before_pixels_are_needed() {
        for region in [
            Region {
                x: 4,
                y: 0,
                width: 1,
                height: 1,
            },
            Region {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1,
            },
            Region {
                x: 0,
                y: 0,
                width: 0,
                height: 1,
            },
            Region {
                x: 0,
                y: 2,
                width: 1,
                height: 2,
            },
        ] {
            assert!(
                CaptureOptions {
                    region: Some(region),
                    ..Default::default()
                }
                .plan(&geometry())
                .is_err()
            );
        }
        for output in [
            ImageDimensions { width: 0, height: 1 },
            ImageDimensions {
                width: u32::MAX,
                height: 1,
            },
            ImageDimensions {
                width: 4096,
                height: 4096,
            },
        ] {
            assert!(
                CaptureOptions {
                    output: Some(output),
                    ..Default::default()
                }
                .plan(&geometry())
                .is_err()
            );
        }
        for (format, quality) in [(ImageFormat::Png, 80), (ImageFormat::Jpeg, 0), (ImageFormat::Jpeg, 101)] {
            assert!(
                CaptureOptions {
                    format,
                    quality: Some(quality),
                    ..Default::default()
                }
                .plan(&geometry())
                .is_err()
            );
        }
    }

    #[test]
    fn defaults_preserve_surface_dimensions_and_edge_pixels() -> Result<()> {
        let plan = CaptureOptions::default().plan(&geometry())?;
        assert_eq!(plan.output, ImageDimensions { width: 4, height: 3 });
        let pixels: Vec<u8> = (0..36).collect();
        let (encoded, mime) = plan.encode(pixels.clone())?;
        assert_eq!(mime, "image/png");
        let mut reader = png::Decoder::new(std::io::Cursor::new(encoded))
            .read_info()
            .map_err(encode_error)?;
        let mut decoded = vec![0; 36];
        reader.next_frame(&mut decoded).map_err(encode_error)?;
        assert_eq!(decoded, pixels);
        Ok(())
    }

    #[test]
    fn crop_scaling_and_input_mapping_use_the_same_pixel_centers() -> Result<()> {
        let plan = CaptureOptions {
            region: Some(Region {
                x: 1,
                y: 1,
                width: 3,
                height: 2,
            }),
            output: Some(ImageDimensions { width: 2, height: 1 }),
            ..Default::default()
        }
        .plan(&geometry())?;
        let (bytes, _) = plan.encode((0..18).collect())?;
        let observation = Observation {
            geometry: geometry(),
            source_region: plan.region,
            image_dimensions: plan.output,
            captured_unix_ms: 0,
            mime_type: "image/png".into(),
            image_base64: STANDARD.encode(&bytes),
        };
        let first = observation.surface_point(&Point { x: 0, y: 0 })?;
        let last = observation.surface_point(&Point { x: 1, y: 0 })?;
        assert_eq!((first.x, first.y, last.x, last.y), (1, 2, 3, 2));
        assert!(observation.surface_point(&Point { x: 2, y: 0 }).is_err());
        assert!(observation.surface_point(&Point { x: -1, y: 0 }).is_err());
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .map_err(encode_error)?;
        let mut decoded = [0; 6];
        reader.next_frame(&mut decoded).map_err(encode_error)?;
        assert_eq!(decoded, [9, 10, 11, 15, 16, 17]);
        Ok(())
    }

    #[test]
    fn jpeg_quality_produces_a_bounded_jpeg_and_rejects_bad_capture_bytes() -> Result<()> {
        let plan = CaptureOptions {
            format: ImageFormat::Jpeg,
            quality: Some(75),
            ..Default::default()
        }
        .plan(&geometry())?;
        assert!(plan.encode(vec![0; 35]).is_err());
        let (bytes, mime) = plan.encode(vec![128; 36])?;
        assert_eq!(mime, "image/jpeg");
        assert!(bytes.starts_with(&[0xff, 0xd8]) && bytes.ends_with(&[0xff, 0xd9]));
        assert!(bytes.len() < 4096);
        Ok(())
    }
}
