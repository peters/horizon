//! A picture of the desktop as plain RGBA pixels, and its crop and scale.
use crate::{Error, ViewOptions};

/// An image of the desktop, or of the part of it that a session shows.
///
/// Each pixel is one `u32` that holds the bytes red, green, blue and alpha in
/// memory order (`u32::from_ne_bytes([r, g, b, a])`). [`Frame::rgba`] is the
/// same memory as plain RGBA8, so a UI can give the pixels to a 4-byte RGBA
/// color type without a copy. Alpha is 255 for each pixel that the server
/// drew. A pixel that it did not draw after the last resize is all zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    size: [usize; 2],
    pixels: Vec<u32>,
}

impl Frame {
    /// # Errors
    /// The pixel count must be width times height.
    pub fn new(size: [usize; 2], pixels: Vec<u32>) -> Result<Self, Error> {
        check_count(size, pixels.len())?;
        Ok(Self { size, pixels })
    }

    /// A frame from RGBA8 bytes, four for each pixel.
    ///
    /// # Errors
    /// The byte count must be four times width times height.
    pub fn from_rgba(size: [usize; 2], rgba: &[u8]) -> Result<Self, Error> {
        let (pixels, rest) = rgba.as_chunks::<4>();
        if !rest.is_empty() {
            return Err(Error::Frame("pixel count does not match the image size"));
        }
        Self::new(size, pixels.iter().map(|pixel| u32::from_ne_bytes(*pixel)).collect())
    }

    /// Width and height in pixels.
    #[must_use]
    pub const fn size(&self) -> [usize; 2] {
        self.size
    }

    /// Row-major pixels, top-left first.
    #[must_use]
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }

    /// The pixels as RGBA8 bytes, without a copy.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        bytemuck::must_cast_slice(&self.pixels)
    }

    #[must_use]
    pub fn into_pixels(self) -> Vec<u32> {
        self.pixels
    }

    /// Crop and scale this desktop for `options`, as a session does before it
    /// publishes an image.
    ///
    /// # Errors
    /// The options must make a layout for this desktop.
    pub fn present(&self, options: ViewOptions) -> Result<Self, Error> {
        let (size, pixels) = present(self.size, &self.pixels, options)?;
        Ok(Self { size, pixels })
    }
}

fn check_count(size: [usize; 2], count: usize) -> Result<(), Error> {
    if size[0].checked_mul(size[1]) == Some(count) {
        Ok(())
    } else {
        Err(Error::Frame("pixel count does not match the image size"))
    }
}

/// Crop and scale a row-major image of `size` for `options`. Each output pixel
/// is the source pixel at its centre. Any pixel type works, so a UI can show a
/// desktop again in its own color type, for example after the options change
/// while no session is connected.
///
/// # Errors
/// The options must make a layout for `size`, and `pixels` must hold width
/// times height pixels.
pub fn present<P: Copy>(size: [usize; 2], pixels: &[P], options: ViewOptions) -> Result<([usize; 2], Vec<P>), Error> {
    check_count(size, pixels.len())?;
    let layout = options.for_desktop(size).layout(size)?;
    if layout.shows_all_of(size) {
        return Ok((size, pixels.to_vec()));
    }
    let width = size[0];
    let columns: Vec<_> = (0..layout.output[0])
        .map(|x| layout.source.x + (2 * x + 1) * layout.source.width / (2 * layout.output[0]))
        .collect();
    let mut output = Vec::with_capacity(layout.output[0] * layout.output[1]);
    for y in 0..layout.output[1] {
        let row = layout.source.y + (2 * y + 1) * layout.source.height / (2 * layout.output[1]);
        for column in &columns {
            output.push(pixels[row * width + *column]);
        }
    }
    Ok((layout.output, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Region;

    #[test]
    fn rgba_bytes_round_trip_in_memory_order() -> Result<(), Error> {
        let rgba = [1, 2, 3, 255, 4, 5, 6, 0];
        let frame = Frame::from_rgba([2, 1], &rgba)?;
        assert_eq!(frame.rgba(), rgba);
        assert_eq!(frame.pixels()[0], u32::from_ne_bytes([1, 2, 3, 255]));
        assert!(Frame::from_rgba([2, 1], &rgba[..7]).is_err(), "a partial pixel");
        assert!(Frame::from_rgba([3, 1], &rgba).is_err(), "too few pixels");
        assert!(Frame::new([usize::MAX, 2], Vec::new()).is_err(), "an overflowing size");
        Ok(())
    }

    #[test]
    fn presenting_crops_and_scales_any_pixel_type() -> Result<(), Error> {
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
        let values: Vec<u8> = (0..8).collect();
        assert_eq!(present([4, 2], &values, options)?, ([1, 1], vec![7]));
        assert_eq!(
            present([4, 2], &values, ViewOptions::default())?,
            ([4, 2], values.clone()),
            "the whole desktop within the limits is a copy"
        );
        assert!(present([4, 2], &values[..7], options).is_err());
        assert!(matches!(
            present([0, 2], &values[..0], options),
            Err(Error::Layout(crate::LayoutError::Limits))
        ));
        Ok(())
    }
}
